#!/usr/bin/env node
// 在**真实微信开发者工具**里跑瘦首包（`-AssetPack remote`），验"容器只在资源包里"这条链路。
//
// 为什么需要它：Node 侧的 harness 假宿主没有 WebGL2（引擎宿主必然 status=failed），
// 远端回落那一段永远走不到；而 -SkipRuntime 的静态门只能证明"文件在、索引对"，
// 证明不了"适配层真的按 packBaseUrl 把容器下下来、挂上、然后 Lua 起得来"。
//
// 做法（不改仓库里的产物：整个 launcher 先拷进临时目录）：
//   1. 起本地静态服务（复用 scripts/dev/pack-server.js，--prefix /pack 还原 CDN 的 key 映射）；
//   2. 把 remote-assets.json 的 packBaseUrl 指向它，urlCheck:false（project.config.json 自带）；
//   3. 适配层取**仓库源码**那份（产物里的副本是上次打包的快照）；
//   4. 往 game.js 顶部注入 thin-probe.js：轮询 __RUSTLUA_DIAG()，把 boot/远端/缓存读数
//      POST 回来（开发者工具的控制台在外面看不到，只能自己上报）；
//   5. cli quit → cli auto --project <临时目录> 冷启动，观察窗口结束后打印 verdict。
//
// 用法：
//   node scripts/dev/wechat-thin-devtools-probe.js
//   node scripts/dev/wechat-thin-devtools-probe.js --project <launcher> --assets <资源包根> --port 8787
// 环境变量：RUSTLUA_DEVTOOLS_CLI（cli.bat 路径）、RUSTLUA_PROBE_TIMEOUT（默认 150s）、
//           RUSTLUA_PROBE_OBSERVE（注入端观察窗口，默认 30s）
'use strict';

const fs = require('fs');
const http = require('http');
const os = require('os');
const path = require('path');
const { exec } = require('child_process');

const CLI = process.env.RUSTLUA_DEVTOOLS_CLI ||
  'D:/Program Files (x86)/Tencent/微信web开发者工具/cli.bat';
const REPORT_PORT = Number(process.env.RUSTLUA_PROBE_PORT || 9422);
const TIMEOUT_MS = Number(process.env.RUSTLUA_PROBE_TIMEOUT || 150000);
const OBSERVE_MS = Number(process.env.RUSTLUA_PROBE_OBSERVE || 30000);

function parseArgs(argv) {
  const args = {
    project: path.resolve(__dirname, '..', '..', 'examples', 'game_jhyt', 'builds', 'wechat-minigame', 'launcher'),
    assets: '',
    port: 8787,
    keep: false,
    remote: true,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    if (key === '--project' && argv[i + 1]) args.project = path.resolve(argv[++i]);
    else if (key === '--assets' && argv[i + 1]) args.assets = path.resolve(argv[++i]);
    else if (key === '--port' && argv[i + 1]) args.port = Number(argv[++i]);
    else if (key === '--keep') args.keep = true;
    else if (key === '--no-remote') args.remote = false;
    else if (key === '--use-packaged-adapter') args.usePackagedAdapter = true;
    else throw new Error('未知参数: ' + key);
  }
  if (!args.remote) return args;
  if (!args.assets) {
    // 约定位置：<发布根>/assets/<pack 目录名>（微信 remote 形态的导出目录，判据是 manifest.txt）。
    const assetsRoot = path.join(path.dirname(args.project), 'assets');
    const inner = fs.existsSync(assetsRoot)
      ? fs.readdirSync(assetsRoot).filter((name) => fs.existsSync(path.join(assetsRoot, name, 'manifest.txt')))
      : [];
    if (inner.length === 0) throw new Error('找不到资源包根（<发布根>/assets/<pack 目录名>/manifest.txt）：' + assetsRoot);
    args.assets = path.join(assetsRoot, inner[0]);
  }
  return args;
}

function sh(cmd) {
  return new Promise((resolve) => exec(cmd, { shell: true, timeout: 120000 }, (e, out, err) =>
    resolve({ e, out: String(out || ''), err: String(err || '') })));
}

// 注入进 launcher 的探针：只读 __RUSTLUA_DIAG()，不改游戏行为。
// 注意：这段是模板字符串的内容，**里面不能再出现反引号**（会提前结束模板）。
const INJECTED_PROBE = `'use strict';
var REPORT_URL = 'http://127.0.0.1:${REPORT_PORT}/report';
// 观察窗口：等"标题画面跑起来 + 后台挂载/懒加载容器"这段时间，太短只能看到首屏骨架。
var OBSERVE_MS = ${OBSERVE_MS};
var started = Date.now();
var sent = false;
// 不用 global：开发者工具的 JS 核心里 global 不保证存在（undefined 时 tick 直接抛，
// 页面看起来正常、上报却永远不来——实测踩过）。
var maxPresented = 0;
var bootFailedAt = 0;
var firstFrameAt = 0;
// 把控制台与宿主错误都截下来一起上报：开发者工具的 Console 在外面看不到，
// 而 boot=failed 这一条本身不含原因（真因在适配层最后几行 log 里）。
var PROBE_LOGS = [];
function probePush(line) {
  try { PROBE_LOGS.push(String(line).slice(0, 300)); if (PROBE_LOGS.length > 160) PROBE_LOGS.shift(); } catch (e) {}
}
['log', 'warn', 'error'].forEach(function (kind) {
  var orig = console[kind];
  console[kind] = function () {
    try {
      probePush(kind + ': ' + Array.prototype.map.call(arguments, function (a) {
        return typeof a === 'string' ? a : ((a && (a.message || a.errMsg)) || String(a));
      }).join(' '));
    } catch (e) {}
    return orig.apply(console, arguments);
  };
});
try {
  if (typeof wx !== 'undefined' && wx && typeof wx.onError === 'function') {
    wx.onError(function (error) { probePush('wx.onError: ' + ((error && (error.message || error.errMsg)) || String(error))); });
  }
} catch (e) {}
function post(payload) {
  if (sent) return;
  sent = true;
  try {
    wx.request({ url: REPORT_URL, method: 'POST', data: payload, timeout: 20000,
      success: function () {}, fail: function () {} });
  } catch (error) { /* 上报失败不影响游戏 */ }
}
function diag() {
  try { if (typeof wx !== 'undefined' && wx && typeof wx.__rustluaDiag === 'function') return wx.__rustluaDiag(); } catch (e) {}
  try { if (typeof __RUSTLUA_DIAG === 'function') return __RUSTLUA_DIAG(); } catch (e) {}
  return null;
}
function sample(elapsed) {
  var d = diag();
  if (!d) return null;
  var host = d.engine_host || {};
  var wasm = d.wasm || {};
  return { boot: String(d.boot || 'unknown'), boot_detail: d.boot_detail,
    host: host.status || 'none', host_reason: host.reason || '',
    host_frames: host.frames, host_presented: host.presented,
    wasm: wasm.status || 'none', wasm_detail: wasm.detail, wasm_lua_bridge: wasm.luaBridge,
    wasm_host_api: wasm.hostApi, wasm_channel: wasm.channel,
    frames: d.frames, remote: d.remote, bundle: d.bundle, pack: d.pack,
    fs_route: d.fs_route, layout: d.layout, images: d.images, version: d.version, elapsed: elapsed };
}
function tick() {
  var elapsed = Date.now() - started;
  var s = sample(elapsed);
  if (s) {
    // boot=failed 不等于"跑不起来"（实测：容器还在预取时启动 Promise 先报红，
    // 而引擎宿主随后照常起来并出帧）。这里只记第一次看到失败的时刻，继续观察到窗口结束。
    if (s.boot === 'failed' && !bootFailedAt) bootFailedAt = elapsed;
    if ((s.host_presented | 0) > maxPresented) maxPresented = s.host_presented | 0;
    if ((s.host_presented | 0) > 0 && !firstFrameAt) firstFrameAt = elapsed;
    if (elapsed > OBSERVE_MS) {
      s.phase = 'done';
      s.max_presented = maxPresented;
      s.boot_failed_at = bootFailedAt;
      s.first_frame_at = firstFrameAt;
      s.logs = PROBE_LOGS.slice(-90);
      s.diag = diag();
      post(s);
      return;
    }
  }
  if (elapsed > 120000) {
    var last = sample(elapsed) || {};
    last.phase = 'boot-timeout';
    last.logs = PROBE_LOGS.slice(-90);
    last.diag = diag();
    post(last);
    return;
  }
  setTimeout(tick, 500);
}
setTimeout(tick, 1500);
`;

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!fs.existsSync(CLI)) throw new Error('找不到开发者工具 cli.bat: ' + CLI + '（用 RUSTLUA_DEVTOOLS_CLI 指定）');
  if (args.remote && !fs.existsSync(path.join(args.assets, 'manifest.txt'))) throw new Error('资源包根没有 manifest.txt: ' + args.assets);

  // 1. 临时工程副本（不污染仓库里的产物）。
  const projectDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rustlua-thin-probe-'));
  fs.cpSync(args.project, projectDir, { recursive: true });
  if (!fs.existsSync(path.join(projectDir, 'game.json'))) throw new Error('工程副本缺 game.json: ' + projectDir);
  const packBaseUrl = `http://127.0.0.1:${args.port}/pack/`;
  if (args.remote) {
    fs.writeFileSync(path.join(projectDir, 'remote-assets.json'), JSON.stringify({
      packBaseUrl, verifyChecksum: true, timeoutMs: 15000,
    }, null, 2));
  } else {
    // 回归形态（容器就在代码包里）：删掉远端配置，确保走的是包内直读那条路。
    try { fs.rmSync(path.join(projectDir, 'remote-assets.json'), { force: true }); } catch (error) { /* 没有就不用删 */ }
  }
  fs.writeFileSync(path.join(projectDir, 'thin-probe.js'), INJECTED_PROBE);
  // 适配层用仓库源码那份：产物里的副本是上次打包的快照，改完还没重打包时不能拿它当"已修好"。
  // 反过来，验收**已发布产物**时要用 --use-packaged-adapter（产物那份是 terser 压过的，语义同源）。
  const adapterSource = path.resolve(__dirname, '..', '..', 'plugins', 'wechat-minigame', 'wechat-adapter.js');
  if (args.usePackagedAdapter) {
    console.log('[probe] 适配层用产物自带那份（不覆盖，验的是实际发出去的字节）');
  } else if (fs.existsSync(adapterSource)) {
    fs.copyFileSync(adapterSource, path.join(projectDir, 'wechat-adapter.js'));
    console.log('[probe] 适配层取自源码: ' + adapterSource);
  }
  const gameJsPath = path.join(projectDir, 'game.js');
  fs.writeFileSync(gameJsPath, "require('./thin-probe.js');\n" + fs.readFileSync(gameJsPath, 'utf8'));
  console.log('[probe] 工程副本: ' + projectDir);
  console.log('[probe] packBaseUrl=' + packBaseUrl + ' 资源包根=' + args.assets);

  // 2. 收报告的本地口。
  let resolveDone = null;
  const done = new Promise((resolve) => { resolveDone = resolve; });
  const reportServer = http.createServer((req, res) => {
    res.writeHead(200, { 'Access-Control-Allow-Origin': '*' });
    res.end('ok');
    if (req.method !== 'POST') return;
    let body = '';
    req.on('data', (chunk) => { body += chunk; });
    req.on('end', () => {
      let parsed = null;
      try { parsed = JSON.parse(body); } catch (error) { return; }
      console.log(`[probe] report phase=${parsed.phase} boot=${parsed.boot} host=${parsed.host} ` +
        `presented=${parsed.host_presented} max=${parsed.max_presented} remote=${JSON.stringify(parsed.remote)}`);
      resolveDone(parsed);
    });
  });
  await new Promise((resolve) => reportServer.listen(REPORT_PORT, '127.0.0.1', resolve));
  console.log(`[probe] 报告口 http://127.0.0.1:${REPORT_PORT}/report`);

  // 3. 起资源服务（CDN 替身）：/pack/<逻辑路径> → <资源包根>/<逻辑路径>。
  //    `--no-remote` 是回归形态（容器在代码包里），这时不起服务，断言也换成"一次远端都不该取"。
  let pack = { requests: [], close: () => Promise.resolve() };
  if (args.remote) {
    const { createPackServer } = require(path.join(__dirname, 'pack-server.js'));
    pack = createPackServer({
      root: args.assets,
      prefix: '/pack',
      log: (line) => {
        try { fs.appendFileSync(path.join(projectDir, 'pack-server.log'), line + '\n'); } catch (error) { /* 日志失败不影响服务 */ }
      },
    });
    const address = await pack.listen(args.port, '127.0.0.1');
    console.log(`[probe] 资源服务 http://127.0.0.1:${address.port}/pack/ root=${args.assets}`);
  } else {
    console.log('[probe] 回归形态（--no-remote）：不起资源服务，容器应当从代码包直读');
  }

  // 4. 冷启动开发者工具。
  console.log('[probe] cli quit（冷启动，让编译缓存与自动化端口都重来）');
  await sh(`"${CLI}" quit`);
  await new Promise((resolve) => setTimeout(resolve, 5000));
  sh(`"${CLI}" auto --project "${projectDir}"`)
    .then((r) => console.log('[probe] cli auto: ' + (r.out + r.err).replace(/\s+/g, ' ').slice(0, 200)));

  const final = await Promise.race([done, new Promise((resolve) => setTimeout(() => resolve(null), TIMEOUT_MS))]);

  // 5. 收尾：关工具、关服务、汇总。
  await sh(`"${CLI}" quit`);
  await pack.close();
  reportServer.close();

  const hits = pack.requests.filter((item) => item.status === 200);
  const misses = pack.requests.filter((item) => item.status !== 200);
  const bytes = hits.reduce((sum, item) => sum + item.bytes, 0);
  const distinct = new Set(hits.map((item) => item.rel));
  const containers = Array.from(distinct).filter((rel) => /\.rlb(\.txt)?$/i.test(rel));
  console.log(`[probe] 资源服务请求: 200=${hits.length} 非200=${misses.length} bytes=${bytes} 容器=${containers.length}`);
  if (misses.length > 0) console.log(`[probe] 非 200 样本: ${misses.slice(0, 5).map((m) => `${m.rel}:${m.status}`).join(', ')}`);

  if (!final) {
    console.log(`wechat_thin_devtools_probe verdict=no-report（${TIMEOUT_MS / 1000}s 内没收到上报；看开发者工具里是否登录/是否打开了本工程）`);
    process.exit(2);
  }
  const fullReportPath = path.resolve(__dirname, '..', '..', 'artifacts', 'logs', 'wechat', 'thin-devtools-report.json');
  try {
    fs.mkdirSync(path.dirname(fullReportPath), { recursive: true });
    fs.writeFileSync(fullReportPath, JSON.stringify(final, null, 2));
  } catch (error) { /* 落盘失败不影响判据 */ }
  console.log('[probe] 完整读数: ' + fullReportPath);

  // 判据只看**可观测的运行证据**：
  //   ① 引擎宿主 running 且真的出过帧（max_presented > 0）
  //   ② 远端形态：容器从远端取过（remote=N>0）且 HTTP 侧真有 .rlb 落地（不是只下了清单/字体）
  //   ③ 回归形态（--no-remote）：远端一次都不该取（remote=0、没有 .rlb 落地）——否则说明它偷偷走了远端
  const presented = final.max_presented | 0;
  const remoteCount = Number((/\bremote=(\d+)/.exec(String(final.remote || '')) || [])[1] || 0);
  const hostRunning = final.host === 'running';
  const ok = args.remote
    ? (hostRunning && presented > 0 && remoteCount > 0 && containers.length > 0)
    : (hostRunning && presented > 0 && remoteCount === 0 && containers.length === 0);
  console.log(`wechat_thin_devtools_probe verdict=${ok
    ? (args.remote ? 'thin-remote-ok-devtools' : 'in-package-ok-devtools')
    : (args.remote ? 'thin-remote-not-proven' : 'in-package-not-proven')} ` +
    `boot=${final.boot}${final.boot_failed_at ? '@' + final.boot_failed_at + 'ms' : ''} host=${final.host} ` +
    `presented=${presented} first_frame_at=${final.first_frame_at || 0}ms remote=${final.remote} ` +
    `containers_served=${containers.length} bytes=${bytes}`);
  if (!ok) {
    console.log(args.remote
      ? '[probe] 判据：host=running + max_presented>0 + 远端取过容器（remote=N>0 且 HTTP 有 .rlb）'
      : '[probe] 判据：host=running + max_presented>0 + 远端一次都没取（remote=0、无 .rlb 落地）');
  }
  if (args.keep) console.log('[probe] 保留临时工程: ' + projectDir);
  process.exit(ok ? 0 : 1);
}

main().catch((error) => {
  console.error('[probe] fatal: ' + String((error && error.stack) || error));
  process.exit(2);
});
