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
//                     outFile 写的是对拍报告（不是 DOM）。见 scripts/lib/web-api-contract.js。
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
// 可选：落定后截一张 PNG（argv[6]）。用 `Page.captureScreenshot` 抓**合成后的页面**，
// 这是唯一能拿到 GPU 画布内容的路子：WebGL 画布默认 `preserveDrawingBuffer:false`，
// 合成之后用 JS 读（`toDataURL`/`getImageData`）拿到的是空白，而合成器里还留着最后一帧。
// 位置参数一律用 `-` 表示「未提供」：PowerShell 向原生程序传参时会**丢掉空字符串**，
// 后面所有参数就会错位一格。这不是洁癖——本轮实测踩到：`-KeyProbe` 的 `1` 落到了 shot 的位置，
// 于是探针静默不跑，而脚本照样 exit 0（最难查的那种失败）。
const argAt = (index) => {
  const value = process.argv[index];
  return !value || value === '-' ? '' : value;
};
const shotFile = argAt(6);
// 可选第二张：把 GPU 画布临时藏掉后再拍一张（argv[7]）。
//
// 这是"两层画面一致性"唯一可控的比较法：同一次运行、同一时刻，一张是"GPU 层 + 底下的
// Canvas2D 层"，另一张是"只有 Canvas2D 层"。两次运行分别取图会因为动画相位不同而不可能比。
const shotUnderFile = argAt(7);
// 关于"合成按键"：`Input.dispatchKeyEvent` 那条 CDP 注入路，本轮之前实测过**进不了**页面的输入
// 状态（`data-rustlua-input="frames=N bits=0"`、`input_edges=0`），而页面**内部**的剧本
// （`?rustlua-drive=jhyt` → `dispatchDriveKey`）同一条事件路径却有效（`bits=60`、`input_edges=13`）。
// 所以"输入进模拟"这件事，正常产物的验法是用 `verify-webgl-bundle.ps1 -Gpu2d -DriveJhyt`。
//
// 但那个结论只覆盖"注入能不能进 **JS 世界**的输入状态"。WP6-04 之后还有一问：
// **Rust 侧自己装的 DOM 监听器收到没有**——而 Rust-only 宿主（Rust-only 宿主；B8 之后这就是**默认**形态）里没有
// JS 世界（页内剧本）根本不存在，只能用页内派发。所以这里加的开关走
// `Runtime.evaluate` + `window.dispatchEvent(new KeyboardEvent(...))`（DOM 标准语义下必然
// 送达 `addEventListener`），并把 `dom_events`（收到几个事件）与 `input_edges`（落到模拟几个边沿）
// **分开记**：两者一起看才知道失败在"没收到"还是"收到了没应用"。
const keyProbe = argAt(8) === '1';
// 真事件探针（argv[12]）：页内派发**真实**的两指 `pointerdown`，验"真实输入源真的在产多指数据"。
// 与 `--input-script` 的 `at2:` 分工不同：那条证明**数据链**装得下两指，这条证明**输入源**在产它。
const pointerProbe = argAt(12) === '1';
// 读到第几帧才算"落定"（0 = 不等）。确定性输入剧本按**帧号**触发，而状态属性每 STATUS_EVERY 帧
// 才写一次、模式落定时刻又取决于浏览器调度 ⇒ 不等到目标帧，读到哪一帧就是碰运气：
// 实测同一条命令两次分别读到 frames=30（正好在按键那一刻，世界还没变）与 frames=240（已生效）。
const settleFrames = Number(argAt(9) || 0) || 0;
// 存档槽探针：从**浏览器侧**独立读一次 `localStorage`，看 Rust 到底有没有把存档写进平台槽。
// 只信 wasm 自己回报的 `commit=true` 不够——那可能只是"自己觉得写成功了"。
// 找不到 `rustlua:save*`（或 payload 不是存档格式）时用退出码 3 报红，由驱动翻成明确的门失败。
const storageProbe = argAt(10) === '1';
// 音频探针（`any` / `none` / `-`）：从**浏览器侧**数 Rust 建出来的 `<audio>` 元素与错误数组。
// 为什么要有独立探针：wasm 自己报的 `audio=N` 是"我以为我请求了"。真正"浏览器收到并开始加载"
// 的证据只在页面里（`__rustluaAudioElements`），自动播放策略的拒绝也在那里（`__rustluaAudioErrors`）。
// `any` = 必须至少有 1 个元素（证明平台通道真的走通）；`none` = 一个都不能有（对照）。
const audioProbe = argAt(11) || '-';
console.log('[cdp] raw argv: ' + JSON.stringify(process.argv.slice(2)));
console.log(
  '[cdp] argv: mode=' + (process.argv[4] || '') + ' shot=' + (shotFile ? 'yes' : 'no') +
    ' keyProbe=' + keyProbe + ' settleFrames=' + settleFrames + ' storageProbe=' + storageProbe
);
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
  // GPU 后端探针（scripts/verify/gpu-web-probe.ps1）：探针页面自己不玩 Lua、不 boot 游戏，
  // 只把 wgpu 的 WebGL2 能力写成 data-gpu-probe。判据与 scene/drive 同款：
  // 落定即可（done/failed 都算落定，红绿由调用方看 JSON 决定）。
  if (mode === 'gpu-probe') {
    const s = stateOf(dom, 'data-gpu-probe');
    return s === 'done' || s === 'failed' ? s : '';
  }
  // 渲染探针（同一脚本的 `-Stage render`）：让**真正的 engine 渲染器**在 WebGL2 上离屏出一帧，
  // 把像素与帧计数写成 data-render-probe。与 gpu-probe 一样只看落定，判据在 .ps1 里。
  if (mode === 'render-probe') {
    const s = stateOf(dom, 'data-render-probe');
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
  // 探针两阶段各有自己的页面文件名（能力探针 index.html / 渲染探针 render.html）；
  // 其余模式（boot/scene/drive/contract）都跑 index.html。按模式挑文件名，而不是只认
  // index.html——否则新页面会被判成"CDP 不可用"（exit 2），看起来像环境问题。
  const pageFile = mode === 'render-probe' ? 'render.html' : 'index.html';
  // Chrome 启动后 /json/list 才可用；最多等 20 秒（首次启动可能较慢）。
  for (let i = 0; i < 40; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/list`);
      const list = await res.json();
      const page = list.find(
        (t) => t.type === 'page' && t.url.includes(pageFile) && t.webSocketDebuggerUrl
      );
      if (page) return page;
    } catch (e) { /* 端口还没起来，继续等 */ }
    await sleep(500);
  }
  return null;
}

/// 落定后真的按几下键，读"宿主收到几个事件"与"落到模拟几个边沿"。
// 真事件探针：页内派发两指 `pointerdown`（`pointerType: "touch"`），让它们同时按住一段时间。
//
// 必须**页内**派发：CDP 的 `Input.dispatchTouchEvent` 在这个 headless 配置里进不了页面的监听器
// （与键盘那条同源，见脚本头部的说明）。手指要真的同时按住若干帧，宿主才会报出多指帧数——
// 那一项是**累计**的，所以之后抬手也不会把它抹回去。
async function runPointerProbe(send) {
  // **投递自检**（与 `keyProbe` 同款，缺了它就无法区分"没送达"与"没记录"）：
  // 页内装一个计数监听器 → 派发一次 → 看收到几次。`hits=0` 说明这条派发路根本没进事件系统，
  // 那么后面"宿主读数=0"就不是宿主的问题。
  const delivery = await send('Runtime.evaluate', {
    expression:
      '(function(){try{var hits=0;var f=function(){hits++;};window.addEventListener("pointerdown",f);' +
      'window.dispatchEvent(new PointerEvent("pointerdown",{pointerType:"touch",pointerId:99,clientX:10,clientY:10,bubbles:true}));' +
      'window.removeEventListener("pointerdown",f);return "hits=" + hits;}catch(e){return "error: " + e;}})()',
    returnByValue: true,
  });
  console.log(
    '[cdp] pointer probe delivery self-test: ' +
      (delivery && delivery.result ? String(delivery.result.value) : '?')
  );
  // `Runtime.evaluate` **不会**因为页内抛错而 reject：所以"没送达"与"构造 PointerEvent 就炸了"
  // 在只看宿主机读数时长得一模一样。每次派发都把页面的异常记下来。
  let dispatchErrors = 0;
  const dispatch = async (type, id, x, y) => {
    const r = await send('Runtime.evaluate', {
      expression:
        'window.dispatchEvent(new PointerEvent("' + type + '", {pointerType: "touch", pointerId: ' +
        id + ', clientX: ' + x + ', clientY: ' + y +
        ', bubbles: true, cancelable: true, isPrimary: ' + (id === 11) + '}))',
      returnByValue: true,
    });
    if (r && r.exceptionDetails) {
      dispatchErrors++;
      if (dispatchErrors <= 2) {
        console.log(
          '[cdp] pointer probe dispatch threw: ' +
            JSON.stringify(r.exceptionDetails).slice(0, 240)
        );
      }
    }
    return r;
  };
  // 第一指先按住（两根手指的**按下先后**就是数组顺序，所以顺序不能反）。
  for (let i = 0; i < 10; i++) {
    await dispatch('pointerdown', 11, 120, 360);
    await sleep(80);
  }
  // 第二指在**第一指仍按住**时按下——这才是"两指同时按"。
  for (let i = 0; i < 10; i++) {
    await dispatch('pointerdown', 12, 900, 500);
    await sleep(80);
  }
  await dispatch('pointerup', 12, 900, 500);
  await dispatch('pointerup', 11, 120, 360);
  await sleep(400);
  // 读数必须**从页面里取**：本探针与 `runKeyProbe` 一样跑在 DOM 落盘**之后**（见调用点），
  // 所以 dump 里那份是探针之前的那一刻，拿它断言等于断言了个寂寞。
  // **先等帧真的推进再读数**：状态属性每 STATUS_EVERY 帧才写一次，读早了拿到的是上一份——
  // 本轮实测踩到：`presented frames=0` 时读到 `multipoint=0`，与"触摸根本没被记录"完全不可区分，
  // 于是我把"探针看不见"读成了"功能不通"（与本会话反复防的"陈旧快照当真值"同一类错）。
  const gpuFrames = async () => {
    const r = await send('Runtime.evaluate', {
      expression: '(document.body ? document.body.getAttribute("data-rustlua-gpu") : "") || ""',
      returnByValue: true,
    });
    const text = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
    const m = /frames=(\d+)/.exec(text);
    return m ? Number(m[1]) : -1;
  };
  const framesBefore = await gpuFrames();
  const framesDeadline = Date.now() + 12000;
  let framesNow = framesBefore;
  while (Date.now() < framesDeadline && framesNow < framesBefore + 20) {
    await sleep(200);
    framesNow = await gpuFrames();
  }
  console.log('[cdp] pointer probe frames ' + framesBefore + '->' + framesNow);  const state = await send('Runtime.evaluate', {
    expression:
      '(document.body ? document.body.getAttribute("data-rustlua-engine-host") : "") || ""',
    returnByValue: true,
  });
  const raw = state && state.result && typeof state.result.value === 'string' ? state.result.value : '';
  const num = (name) => {
    const m = new RegExp('"' + name + '":(\\d+)').exec(raw);
    return m ? Number(m[1]) : -1;
  };
  console.log(
    '[cdp] pointer probe multipoint=' + num('multipoint_frames') +
      ' pointer_count=' + num('pointer_count') + ' seen_frames=' + num('seen_frames') +
      ' dispatch_errors=' + dispatchErrors
  );
}

async function runKeyProbe(send) {
  const read = async () => {
    const r = await send('Runtime.evaluate', {
      expression: 'document.body ? (document.body.getAttribute("data-rustlua-gpu") || "") : ""',
      returnByValue: true,
    });
    const state = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
    const num = (name) => {
      const m = new RegExp(name + '=(\\d+)').exec(state);
      return m ? m[1] : '?';
    };
    return {
      frames: Number(num('frames')),
      dom_events: num('dom_events'),
      input_edges: num('input_edges'),
      raw: state,
    };
  };
  // 状态属性**不是每帧写的**（宿主每 STATUS_EVERY 帧刷一次），所以按键之后必须等到下一次刷新
  // 才能读到新值——否则读到的是旧快照，看起来就像"事件没到"。这是本轮实际踩到的第二次误判。
  const readFresh = async (sinceFrames) => {
    const deadline = Date.now() + 6000;
    let last = await read();
    while (Date.now() < deadline && last.frames < sinceFrames + 20) {
      await sleep(200);
      last = await read();
    }
    return last;
  };
  const before = await read();
  // 先自测"页内派发到底有没有送到 window 上的监听器"：这一步把"浏览器没送达"与
  // "Rust 侧监听器没装上"分清楚。两者都会表现为 `dom_events` 不变，但修法完全不同。
  const delivery = await send('Runtime.evaluate', {
    expression:
      '(function(){var hits=0;var f=function(){hits++;};window.addEventListener("keydown",f);' +
      'window.dispatchEvent(new KeyboardEvent("keydown",{key:"w",code:"KeyW",bubbles:true}));' +
      'document.dispatchEvent(new KeyboardEvent("keydown",{key:"w",code:"KeyW",bubbles:true}));' +
      'window.removeEventListener("keydown",f);return hits;})()',
    returnByValue: true,
  });
  console.log(
    '[cdp] key probe: in-page delivery self-test hits=' +
      (delivery && delivery.result ? delivery.result.value : '?')
  );
  const dispatch = async (type) =>
    send('Runtime.evaluate', {
      expression:
        'window.dispatchEvent(new KeyboardEvent("' + type + '", {key: "w", code: "KeyW", ' +
        'keyCode: 87, which: 87, bubbles: true}))',
      returnByValue: true,
    });
  // 按住一段时间再松开：一步固定步长要能观察到，且要能区分"按下边沿"与"按住状态"。
  for (let i = 0; i < 12; i++) {
    await dispatch('keydown');
    await sleep(80);
  }
  const held = await readFresh(before.frames);
  for (let i = 0; i < 6; i++) {
    await dispatch('keyup');
    await sleep(80);
  }
  await sleep(400);
  const after = await readFresh(held.frames);
  // `world=` 一起打出来：这条探针原本只数"事件到没到、边沿有几条"，数不到"世界动没动"。
  // 键盘纵向被两个通道抵消那一笔（见 web-runtime.js 的 snapshot()）正好是**边沿都在、位移为零**
  // 的形态，只看计数会一路绿灯。判据仍然不在这里——先把读数打得出来，归因留给台账。
  // 口径提醒：`data-rustlua-gpu` 是**空格**分隔的，`[^;]*` 会把后段一起吞进来，所以打出来的
  // `world` 段是连着 `sched_frames=` 那一串的——按空格拆着读（本笔现场就是这么读的）。
  const worldOf = (state) => {
    const m = /world=([^;]*)/.exec(state || '');
    return m ? m[1].trim() : 'world=absent';
  };
  console.log(
    '[cdp] key probe: frames ' + before.frames + ' -> ' + held.frames + ' -> ' + after.frames +
      ' ; dom_events ' + before.dom_events + ' -> ' + held.dom_events + ' -> ' + after.dom_events +
      ' ; input_edges ' + before.input_edges + ' -> ' + after.input_edges +
      ' ; world ' + worldOf(before.raw) + ' -> ' + worldOf(held.raw) + ' -> ' + worldOf(after.raw)
  );
}

/// 从浏览器侧独立读平台存档槽（`localStorage` 里 `rustlua:save*` 那几条）。
///
/// 为什么要在**浏览器侧**读、而不是只看 wasm 报的 `commit=true`：那是"自己说自己写成功了"。
/// 平台槽是这段代码唯一的落盘通道，只有从外面读一眼才能证明字**真的**进去了。
/// 顺带核格式：引擎写的 payload 必须与旧运行时同格式（`save_version=` 开头），
/// 否则"换了通道，旧档读不出来"这件事会一直藏着。
async function runStorageProbe(send) {
  const r = await send('Runtime.evaluate', {
    expression:
      '(function(){try{var out=[];for(var i=0;i<localStorage.length;i++){' +
      'var k=localStorage.key(i);if(!k||k.indexOf("rustlua:save")!==0)continue;' +
      'var v=localStorage.getItem(k)||"";' +
      'out.push(k+"="+v.length+"hdr:"+(v.indexOf("save_version=")===0?"ok":"bad"));}return out.join(",");}' +
      'catch(e){return "error:"+e;}})()',
    returnByValue: true,
  });
  const summary = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
  console.log('[cdp] storage probe: ' + (summary || '(no rustlua:save entry)'));
  if (!summary || summary.indexOf('rustlua:save') !== 0) {
    console.log('[cdp] storage probe FAILED: 平台槽里没有 rustlua:save 条目');
    return 3;
  }
  if (summary.indexOf('hdr:ok') < 0) {
    console.log('[cdp] storage probe FAILED: 存档 payload 不是引擎/旧运行时同格式（缺 save_version= 头）');
    return 3;
  }
  return 0;
}

/// 从浏览器侧数 Rust 侧建出来的音频元素（`__rustluaAudioElements`）。
///
/// 这个探针回答的是"平台音频通道到底通没通"：元素在、`src` 对、`paused=false` ⇒ 浏览器
/// 真的开始加载/播放了。`__rustluaAudioErrors` 里的内容是最常见的"没声音"原因
/// （自动播放策略 `NotAllowedError`），必须能被看见，而不是让游戏静悄悄无声。
async function runAudioProbe(send, expect) {
  const r = await send('Runtime.evaluate', {
    expression:
      '(function(){try{var els=window.__rustluaAudioElements||null;' +
      'var errs=window.__rustluaAudioErrors||[];' +
      'var count=els?els.length:0;var rows=[];' +
      'for(var i=0;i<count;i++){var e=els[i]||{};' +
      'rows.push(String(e.src||"?")+"|paused="+String(e.paused)+"|loop="+String(e.loop)+"|vol="+String(e.volume));}' +
      'return "elements="+count+" errors="+errs.length+(errs.length?(" ["+errs.join(" ; ")+"]"):"")+' +
      '(rows.length?(" "+rows.join(" ")):"");}catch(e){return "error:"+e;}})()',
    returnByValue: true,
  });
  const summary = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
  console.log('[cdp] audio probe (' + expect + '): ' + summary);
  const count = Number((summary.match(/elements=(\d+)/) || [])[1] || 0);
  if (expect === 'any' && count < 1) {
    console.log('[cdp] audio probe FAILED: 浏览器侧一个音频元素都没有（平台通道没走到）');
    return 4;
  }
  if (expect === 'none' && count > 0) {
    console.log('[cdp] audio probe FAILED: 对照运行里不该有音频元素，实际 ' + count + ' 个');
    return 4;
  }
  return 0;
}

async function captureShot(send, file, underFile) {
  if (!file) return;
  // 若页面在跑 GPU 宿主（`data-rustlua-gpu`），等它把帧数推上去再拍：那条属性从第 1 帧就开始写，
  // 而首帧含一次性建图集/建管线——拍首帧等于拍半成品，与"稳态画面"不是一回事。
  // 没跑 GPU 宿主（纯 Canvas2D）时这里立刻返回，不影响老流程。
  const shotDeadline = Date.now() + 20000;
  while (Date.now() < shotDeadline) {
    let state = '';
    try {
      const r = await send('Runtime.evaluate', {
        expression: 'document.body ? (document.body.getAttribute("data-rustlua-gpu") || "") : ""',
        returnByValue: true,
      });
      state = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
    } catch (e) { /* 页面可能在忙，下一轮再试 */ }
    if (!state) break;
    const m = /presented frames=(\d+)/.exec(state);
    if (m && Number(m[1]) >= 60) {
      console.log('[cdp] gpu frames settled for shot: ' + m[0]);
      break;
    }
    await sleep(500);
  }
  try {
    const shot = await send('Page.captureScreenshot', { format: 'png', fromSurface: true });
    if (shot && shot.data) {
      fs.writeFileSync(file, Buffer.from(shot.data, 'base64'));
      console.log('[cdp] screenshot written: ' + file);
    } else {
      console.log('[cdp] screenshot empty');
    }
    if (underFile) {
      // 藏掉 GPU 画布，露出底下的 Canvas2D 层，再拍一张。用 JS 改样式而不是改产物：
      // 产物里一行都不动，比较的是**同一份代码在同一次运行里的两层**。
      const hide = await send('Runtime.evaluate', {
        expression: '(function(){var c=document.getElementById("rustlua-gpu-canvas");' +
          'if(!c)return "no-canvas";c.style.display="none";return "hidden";})()',
        returnByValue: true,
      });
      console.log('[cdp] gpu canvas: ' + (hide && hide.result ? hide.result.value : '?'));
      await sleep(600);
      const under = await send('Page.captureScreenshot', { format: 'png', fromSurface: true });
      if (under && under.data) {
        fs.writeFileSync(underFile, Buffer.from(under.data, 'base64'));
        console.log('[cdp] under-layer screenshot written: ' + underFile);
      }
      await send('Runtime.evaluate', {
        expression: '(function(){var c=document.getElementById("rustlua-gpu-canvas");' +
          'if(c)c.style.display="";return "restored";})()',
        returnByValue: true,
      });
    }
  } catch (e) {
    console.log('[cdp] screenshot failed: ' + e);
  }
}

async function main() {
  const page = await findPage();
  if (!page) {
    console.log('[cdp] no page target found on port ' + port);
    process.exit(2);
  }
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  let msgId = 0;
  let consoleLines = 0;
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
      return;
    }
    // 页面侧的报错就是**诊断证据**，必须留住：wasm 的 panic/abort 不走 DOM，
    // 只在控制台留一行；不采集它，"引擎卡死"与"引擎炸了"在 dump 里长得一模一样。
    if (msg.method === 'Runtime.consoleAPICalled') {
      const args = (msg.params.args || [])
        .map((a) => (a.value !== undefined ? a.value : a.description || a.type))
        .join(' ');
      if (consoleLines++ < 200) console.log('[console.' + msg.params.type + '] ' + args);
      return;
    }
    if (msg.method === 'Runtime.exceptionThrown') {
      const d = msg.params.exceptionDetails || {};
      if (consoleLines++ < 200) console.log('[exception] ' + (d.exception?.description || d.text || ''));
      return;
    }
    if (msg.method === 'Log.entryAdded') {
      const e = msg.params.entry || {};
      if (consoleLines++ < 200) console.log('[log.' + e.level + '] ' + (e.text || ''));
    }
  };
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error('websocket connect failed'));
  });
  // 打开控制台/异常/浏览器日志三条事件流（页面加载之前的报错靠 Log 兜住）。
  try { await send('Runtime.enable', {}); } catch (e) { /* 老版本可能不支持，不影响主流程 */ }
  try { await send('Log.enable', {}); } catch (e) { /* 同上 */ }

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
        console.log('[cdp] ' + mode + ' settled: ' + outcome);
        // 确定性剧本/世界查询：等到目标帧再写 DOM。
        // 为什么必须在**写文件之前**等：驱动断言的是这份 DOM 里的 `data-rustlua-gpu` 读数，
        // 而它在落定瞬间可能还停在按键那一帧（见 `settleFrames` 的注释）。
        if (settleFrames > 0) {
          const gpuDeadline = Date.now() + 15000;
          let reached = '';
          while (Date.now() < gpuDeadline) {
            let state = '';
            try {
              const r = await send('Runtime.evaluate', {
                expression: 'document.body ? (document.body.getAttribute("data-rustlua-gpu") || "") : ""',
                returnByValue: true,
              });
              state = r && r.result && typeof r.result.value === 'string' ? r.result.value : '';
            } catch (e) { /* 页面可能在忙，下一轮再试 */ }
            const m = /frames=(\d+)/.exec(state);
            if (m && Number(m[1]) >= settleFrames) { reached = m[0]; break; }
            await sleep(200);
          }
          console.log('[cdp] readout settle: ' + (reached || 'timeout(>= ' + settleFrames + ' frames)'));
          try {
            const r = await send('Runtime.evaluate', {
              expression: 'document.body ? document.body.outerHTML : ""',
              returnByValue: true,
            });
            if (r && r.result && typeof r.result.value === 'string' && r.result.value) dom = r.result.value;
          } catch (e) { /* 保持落定那一份 DOM */ }
        }
        fs.writeFileSync(outFile, dom, 'utf8');
        await captureShot(send, shotFile, shotUnderFile);
        if (keyProbe) await runKeyProbe(send);
if (pointerProbe) await runPointerProbe(send);
        // 存档槽探针放在最后：它不改页面状态，也不影响截图/按键读数。
        if (storageProbe) {
          const code = await runStorageProbe(send);
          if (code !== 0) process.exit(code);
        }
        if (audioProbe !== '-') {
          const code = await runAudioProbe(send, audioProbe);
          if (code !== 0) process.exit(code);
        }
        process.exit(0);
      }
    }
    await sleep(pollMs);
  }
  if (lastDom) fs.writeFileSync(outFile, lastDom, 'utf8');
  // 探针放在**超时路径也要跑**：引擎宿主独占这一帧时（原子模式默认），场景/剧本读数没有写者、
  // `settled` 永远不会真，于是 harness 走超时分支——而探针本来只写在落定分支里，就永远跑不到。
  // 探针的读数是从**页面**里取的（不是 dump），所以页面还活着就够。
  if (pointerProbe) {
    try {
      await runPointerProbe(send);
    } catch (probeError) {
      console.log('[cdp] pointer probe failed: ' + probeError);
    }
  }
  console.log('[cdp] timeout waiting for ' + mode + ' (last boot=' + stateOf(lastDom, 'data-rustlua-boot') + ')');
  process.exit(1);
}

main().catch((e) => {
  console.log('[cdp] fatal: ' + e);
  process.exit(2);
});
