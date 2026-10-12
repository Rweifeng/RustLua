// docs/65 §12 P0-① 六问的**模拟器侧**探针。
//
// 通道选型（都是实测逼出来的）：官方 miniprogram-automator 在 compileType=game 的运行时里
// 只有 `Tool.*` 域应答；`App.callFunction` / `App.callWxMethod` / `App.enableLog` 全部超时
// （2026-10-12，IDE 2.02.2608080 / SDK 2.32.3）。所以代码注入走不了，读数改走**产物自己的腿**：
//   launcher/p0-probe.js（临时注入物）开启适配层的探针/宿主开关并采集六问读数，
//   经 `wx.request` POST 回本脚本监听的 http://127.0.0.1:9422/report
//   （launcher/project.config.json 的 `urlCheck:false` 放行本地域名）。
// 本脚本负责：起监听 → `cli quit`/`cli auto` 冷启动项目 → 收报告 → 打 verdict 行。
// ④（前后台恢复）与 ⑥（设备内存预算）模拟器没有可信通道，如实登记 device-pending。
//
// 用法：node scripts/wechat-devtools-probe.js
// 环境：RUSTLUA_DEVTOOLS_CLI（cli.bat 路径）、RUSTLUA_DEVTOOLS_PROJECT（launcher 目录）、
//       RUSTLUA_PROBE_PORT（默认 9422）、RUSTLUA_PROBE_TIMEOUT（默认 300000ms）。
'use strict';

const http = require('http');
const path = require('path');
const { exec } = require('child_process');

const CLI = process.env.RUSTLUA_DEVTOOLS_CLI ||
  'D:/Program Files (x86)/Tencent/微信web开发者工具/cli.bat';
const PROJECT = process.env.RUSTLUA_DEVTOOLS_PROJECT || path.resolve(
  __dirname, '..', 'examples', 'game_jhyt', 'builds', 'wechat-minigame', 'launcher');
const PORT = Number(process.env.RUSTLUA_PROBE_PORT || 9422);
const TIMEOUT_MS = Number(process.env.RUSTLUA_PROBE_TIMEOUT || 360000);
const PRESENT_TARGET = 300;

function sh(cmd) {
  return new Promise((r) => exec(cmd, { shell: true }, (e, out) => r({ e, out: String(out).slice(0, 300) })));
}

async function main() {
  const reports = [];
  let resolveDone = null;
  const done = new Promise((r) => { resolveDone = r; });
  const server = http.createServer((req, res) => {
    res.writeHead(200);
    res.end('ok');
    if (req.method === 'POST') {
      let body = '';
      req.on('data', (c) => { body += c; });
      req.on('end', () => {
        let parsed;
        try { parsed = JSON.parse(body); } catch (e) { return; }
        reports.push(parsed);
        console.log(`[probe] report phase=${parsed.phase} ` + JSON.stringify(parsed).slice(0, 220));
        if (parsed.phase === 'done' || parsed.phase === 'boot-failed' || parsed.phase === 'probe-threw') {
          resolveDone(parsed);
        }
      });
      return;
    }
    // GET /report/fail?err=… —— wx.request 通道自身的失败回信（别让它静默）。
    const m = /\/report\/fail\?err=([^&]*)/.exec(req.url || '');
    if (m) console.log('[probe] channel-fail err=' + decodeURIComponent(m[1]).slice(0, 160));
  });
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  console.log(`[probe] listening http://127.0.0.1:${PORT}/report`);

  console.log('[probe] cli quit（冷启动，让自动化端口与编译缓存都重来）');
  await sh(`"${CLI}" quit`);
  await new Promise((r) => setTimeout(r, 8000));
  sh(`"${CLI}" auto --project "${PROJECT}"`)
    .then((r) => console.log('[probe] cli auto: ' + r.out.replace(/\s+/g, ' ').slice(0, 160)));

  const timer = new Promise((r) => setTimeout(() => r(null), TIMEOUT_MS));
  const final = await Promise.race([done, timer]);
  server.close();

  if (!final) {
    console.log(`wechat_devtools_probe verdict=no-report（${TIMEOUT_MS / 1000}s 内没收到 POST；` +
      '检查 launcher/p0-probe.js 是否还在、IDE 是否真的冷启了本项目）');
    process.exit(2);
  }
  if (final.phase === 'boot-failed' || final.phase === 'probe-threw') {
    console.log(`wechat_devtools_probe verdict=${final.phase} detail=` +
      `${final.error || (final.boot && final.boot.wasm && final.boot.wasm.detail) || ''}`);
    process.exit(2);
  }

  const q1 = final.q1 || {};
  const q2 = final.q2 || {};
  const q3 = final.q3 || {};
  const q5 = final.q5 || {};
  console.log(`wechat_devtools_probe boot=wasm:${final.boot && final.boot.wasm ? final.boot.wasm.status + '/' + (final.boot.wasm.channel || '') : '?'}`);
  if (final.q0) console.log('[probe] q0 gl-preflight: ' + JSON.stringify(final.q0));
  console.log(`wechat_devtools_probe q1_canvas_host=${q1.status}${q1.reason ? '(' + q1.reason + ')' : ''}${q1.source ? ' source=' + q1.source : ''}`);
  console.log(`wechat_devtools_probe q2_engine_host_begin=${q2.status}${q2.reason ? '(' + q2.reason + ')' : ''}${q2.source ? ' source=' + q2.source : ''}`);
  console.log(`wechat_devtools_probe q3_presented=${q3.presented}/${PRESENT_TARGET} reached=${q3.reached ? 'yes' : 'no'} span_ms=${q3.span_ms} failed_presents=${q3.failed_presents} status=${q3.status}`);
  console.log(`wechat_devtools_probe q4_foreground_restore=device-pending（模拟器无前后台切换通道，automator App.* 域在小游戏不响应）`);
  console.log(`wechat_devtools_probe q5_eh=${q5.ehProbe}`);
  console.log(`wechat_devtools_probe q6_mem_budget=device-pending wasm_bytes=${final.boot && final.boot.wasm ? final.boot.wasm.bytes : '?'}`);
  // ehProbe 的读数是整句 `eh_probe=accepted（…）`，只能按"含 accepted"判，不能全等。
  const ok = q1.status === 'ok' && q2.status === 'running' && q3.reached && String(q5.ehProbe || '').includes('accepted');
  console.log(`wechat_devtools_probe verdict=${ok ? 'gpu-route-ok-simulator' : 'gpu-route-not-proven'}（六问里 ④⑥ 需真机；模拟器读数不外推到设备）`);
  console.log('[probe] full json: ' + JSON.stringify(final));
  process.exit(ok ? 0 : 1);
}

main().catch((e) => { console.error('[probe] fatal:', e && e.stack || e); process.exit(2); });
