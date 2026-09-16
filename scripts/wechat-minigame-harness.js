// 微信小游戏发布包的无头验证器（scripts/wechat-minigame-harness.js）
//
// 现状说明：CI/本机没有微信开发者工具与真机，因此这里用 Node 伪造一层 `wx` API，
// 加载发布包里的 game.js → wechat-adapter.js → web-runtime.js，真实执行：
//   - 读取 pack/ 资源（manifest / project.toml / 场景 / Lua 模块 / 贴图）
//   - 用 Fengari 跑 Lua 主脚本并驱动若干帧
//   - 触摸/键盘事件桥接
// 断言的是"这些东西真的被执行了"，而不是"文件存在"。
//
// 用法：node scripts/wechat-minigame-harness.js <发布目录> [--frames N]
//   --drive-new-game          走玩家真实路径（标题按确认 → scene.load 换场景）
//   --expect-runtime-spawns   断言运行期生成了实体（alive > 场景声明数）
//   --drive-move N            世界起来后按住方向键 N 帧，断言主角坐标真的变化
// 退出码：0 通过、1 失败（失败原因打印到 stderr）。
'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');
const vm = require('vm');
const http = require('http');
// 真机代码包规则（后缀白名单 / 被编译的文件）：与构建期、适配层判据同源。
const rules = require('./code-package-rules.js');

// async：远端模式需要在 require(game.js) **之前**把本地服务起好并落配置，
// 而适配层在 start() 时就会读 remote-assets.json（同步时机）。
async function main(argv) {
  const root = path.resolve(argv[0] || '.');
  // 未处理的 Promise 拒绝必须被抓住。分包失败改走"可见但非致命"的通道之后，最危险的回归就是
  // 某个内部 Promise 悄悄 reject 掉——小游戏里那表现为彻底的静默（没有 unhandledRejection 提示），
  // 而"失败必须响亮"正是这一形态的核心承诺之一。
  const unhandledRejections = [];
  process.on('unhandledRejection', (reason) => {
    unhandledRejections.push(String((reason && reason.message) || reason));
  });
  let frames = 12;
let noFrames = false;
  // 模拟不同宿主对**代码包/分包路径形式**的接受程度：开发者工具、真机、不同基础库并不一致，
  // 适配层必须都能读通（--wx-path-style root|slash|sub-root|no-utf8）。
  let wxPathStyle = 'root';
  let wxPathStyleGiven = false;
  // 远端回落模式：
  //   auto    = 包内**读不到** pack/（整个资源包都在远端），应有远端证据并通过
  //   thin    = 瘦首包：pack/ 只留骨架（game.js/project.toml/settings.toml/manifest.txt），
  //             重资源全部走远端；此时包内读取必须**照样命中**（包内优先），远端只补缺
  //   corrupt = 应被校验和拦下；down = 应快速失败（不许挂死）
  let remoteMode = 'off';
  // 瘦首包/远端服务用的资源包根目录（内含 pack/）；默认就是发布目录本身。
  let remoteRoot = null;
  // 模拟"宿主连 Image.src 都不让改写"的基础库：用于验证适配层在远端模式下
  // **响亮失败**（而不是静默丢贴图），以及包内完整时的容忍行为。
  let frozenImageSrc = false;
  // 模拟微信基础库的 Image 形态：`src` 是**原型上的** class 访问器（实例上没有 own 描述符）。
  // 只看 own 描述符的适配层会静默退化成 `image._src = value`，宿主永远不开始加载 →
  // 启动卡死 + 黑屏 + 0 error（这正是实测踩到的开发者工具黑屏）。
  let prototypeImageSrc = false;
  // 模拟基础库 2.32.3 的 `k$` 包装类形态：`src`、`onload`、`onerror` **全在原型上**，
  // 而且真实 image 的回调是**构造时**接成 `this.__onload` 的（宿主的 setter 把回调存进私有字段）。
  // 只看"能不能写 src"是不够的：回调桥接在私有字段上时，用 own property 盖住 `onload`
  // 就等于把桥拆了 —— 贴图加载成功也传不回运行时，症状同样是"卡死 + 0 错误 + 全黑"。
  let imageCallbackBridge = false;
  // 模拟"宿主收下了 src 却永远不回调"的基础库：src 写进去了（连 pending 都记了），
  // 但既没有 onload 也没有 onerror。运行时的贴图 Promise 于是永不 settle——
  // 这正是启动看门狗要覆盖的场景（无此形态时看门狗自己也无法被验证）。
  let silentImageSrc = false;
  // 期望"启动卡死"是**预期行为**：此时断言适配层的看门狗确实报了警并画到了屏幕上。
  let expectBootStall = false;
  // 看门狗超时（毫秒），通过存储注入给适配层（见 wechat-adapter.js 的 p2_boot_timeout_ms）。
  let bootTimeoutMsOverride = 0;
  // harness 自己的启动等待上限，默认 30s；卡死用例要把它压到几秒，否则一条用例跑 30 秒进不了质量门。
  let bootDeadlineOverride = 0;
  // 模拟微信开发者工具的壳：它自带半套 DOM，`document` 已被占用且**不可写**——
  // 这正是 "TypeError: document.getElementById is not a function" 的现场（实测报错）。
  //   stub    = 不可写的残缺 document（缺 getElementById/createElement/body/fonts）
  //   frozen  = 不可写**且冻结**的 document（补不齐 → 适配层必须响亮失败）
  // --host-window separate = 宿主另有一个独立 window 对象（不是 globalThis）
  let hostDocument = '';
  let hostWindowMode = '';
  // 上屏画布与 wx.createCanvas() 返回值不一致的宿主形态（见下面的 offscreenCanvas 说明）。
  // --debug-overlay：开启适配层的屏幕自证覆盖层（走 wx.getStorageSync('p2_debug_overlay')）。
  let hostCanvasMode = '';
  let debugOverlay = false;
  // 期望"贴图拿不到 → 屏幕上出现红色警示条"（配合 --remote down 使用）。
  let expectImageBand = false;
  // 走**玩家的真实路径**：在标题界面按确认（空格）→ `title_menu.update()` →
  // `on_click('BtnStart')` → `scene.load("manor")`。只跑标题界面的用例（默认）永远
  // 走不到这条路径，所以"600 帧全绿但点开始新游戏就崩"曾经是覆盖盲区：
  // 引擎里 `scene.load` 是**排队的场景切换**（switch 后重跑 on_create），
  // 而 web 运行时若不换场景数据，`update()` 会立刻按"已在玩法地图"分支执行。
  let driveNewGame = false;
  let driveFrame = 3; // 第几帧按下确认键（前面留几帧让标题界面完成首帧绘制）
  // 断言"运行期生成了实体"（alive > 场景声明数）：敌人/NPC 这类不在场景里写死的角色
  // 全靠 entity.spawn + sprite.set + animation.attach 这条链路，它坏了画面"不报错但没人"。
  let expectRuntimeSpawns = false;
  // 世界确认之后，按住**运行时自己从触摸摇杆派发出来的键**若干帧，断言主角坐标真的变了。
  // 这条覆盖的是整条宿主契约链路：输入 → 游戏 → physics.set_position/transform.set →
  // 回写通道 → 渲染器。游戏把摇杆键解释成什么动作是游戏的事，所以按哪个键由运行时产出，
  // 不由用例硬编码。
  let driveMove = 0;
  // 分包挂载行为——新发布形态（主包 + 单分包）的核心**宿主语义**，与 `--wx-path-style` 正交：
  //   auto      = 默认。game.json 真的声明了 pack 分包时按"未挂载则读不到 pack/"模拟，并成功挂载
  //   off       = 关掉分包模拟（声明了分包也当普通目录读）——用来把"方言"这一轴单独隔离出来
  //   ok        = 显式要求"声明了分包 + 成功挂载"（分包包上等价于 auto，便于用例自述意图）
  //   never     = 永不挂载（fail），pack/ 从头到尾读不到 → 断言首屏照常 + 失败可见且响亮
  //   delay     = 第 N 帧才挂载（N = --subpackage-delay-frames），用来证明"首屏不等分包"
  //   fail-once = 首次失败、冷却后重试成功（验证失败不会变成永久瘫痪）
  let subpackageMode = 'auto';
  let subpackageDelayFrames = 0;
  let expectPackFailure = false;
  // 模拟"宿主不给读代码包里的某个文件"——**真机实测**：
  //   readFileSync('game.json') 一律 permission denied（平台配置不向 FileSystemManager 开放，
  //   与小程序侧 app.json 同理）。这条必须能复现，否则"适配层拿它当判据"这类缺陷在无头测试里全绿。
  // 逗号分隔的路径列表（相对代码包根，如 `game.json` 或 `mainpack/project.toml`）。
  const denyCodeFiles = [];
  // `--expect-no-read X`：拒绝读 X，并**要求适配层一次都没读它**。用来钉死"适配层不得再把某个
  // 文件当判据"——实测踩过：拿 `game.json` 当"要不要挂分包"的判据，而它在真机上读不到，
  // 于是分包静默不挂。这个方向与 `--deny-code-file` 相反（那个要求"确实挡住过"，证明用例非空转）。
  const expectNoReadFiles = [];
  // `--deny-code-dir <前缀>`：**真机实测规则**——代码包里只有"根目录文件"与"已声明的分包 root 下"
  // 的可读，其余子目录一律 `permission denied`（文件明明在项目里、也不在 ignore 列表里）。
  // 与 `--deny-code-file` 的区别：这个是**按目录前缀**拒绝，用来复现真机的整体行为，不是单点。
  // 校准依据（同一份日志）：`remote-assets.json`（磁盘上也没有）报 `not found`，
  // 而 `mainpack/project.toml`（磁盘上有）报 `permission denied` —— 两种报错必须分开建模，
  // 否则"读不到"这件事在无头测试里永远含糊。
  const denyCodeDirs = [];
  const hostStubs = {};
  for (let i = 1; i < argv.length; i++) {
    if (argv[i] === '--frames' && argv[i + 1]) frames = Number(argv[++i]) || 12;
if (argv[i] === '--no-frames') noFrames = true;
    else if (argv[i] === '--wx-path-style' && argv[i + 1]) { wxPathStyle = String(argv[++i]); wxPathStyleGiven = true; }
    else if (argv[i] === '--remote' && argv[i + 1]) remoteMode = String(argv[++i]);
    else if (argv[i] === '--remote-root' && argv[i + 1]) remoteRoot = path.resolve(String(argv[++i]));
    else if (argv[i] === '--image-src-frozen') frozenImageSrc = true;
    else if (argv[i] === '--image-src-prototype') prototypeImageSrc = true;
    else if (argv[i] === '--image-callback-bridge') imageCallbackBridge = true;
    else if (argv[i] === '--image-src-silent') silentImageSrc = true;
    else if (argv[i] === '--expect-boot-stall') expectBootStall = true;
    else if (argv[i] === '--boot-timeout-ms' && argv[i + 1]) bootTimeoutMsOverride = Number(argv[++i]) || 0;
    else if (argv[i] === '--boot-deadline-ms' && argv[i + 1]) bootDeadlineOverride = Number(argv[++i]) || 0;
    else if (argv[i] === '--host-document' && argv[i + 1]) hostDocument = String(argv[++i]);
    else if (argv[i] === '--host-window' && argv[i + 1]) hostWindowMode = String(argv[++i]);
    else if (argv[i] === '--host-canvas' && argv[i + 1]) hostCanvasMode = String(argv[++i]);
    else if (argv[i] === '--debug-overlay') debugOverlay = true;
    else if (argv[i] === '--expect-image-band') expectImageBand = true;
    else if (argv[i] === '--drive-new-game') driveNewGame = true;
    else if (argv[i] === '--expect-runtime-spawns') expectRuntimeSpawns = true;
    else if (argv[i] === '--drive-move' && argv[i + 1]) driveMove = Math.max(0, Number(argv[++i]) || 0);
    else if (argv[i] === '--drive-frame' && argv[i + 1]) driveFrame = Math.max(0, Number(argv[++i]) || 0);
    else if (argv[i] === '--subpackage' && argv[i + 1]) subpackageMode = String(argv[++i]);
    else if (argv[i] === '--subpackage-delay-frames' && argv[i + 1]) subpackageDelayFrames = Math.max(0, Number(argv[++i]) || 0);
    else if (argv[i] === '--expect-pack-failure') expectPackFailure = true;
    else if (argv[i] === '--deny-code-file' && argv[i + 1]) {
      // 可重复给，也可一次给逗号分隔的多个
      for (const item of String(argv[++i]).split(',')) {
        const trimmed = item.trim();
        if (trimmed) denyCodeFiles.push(trimmed);
      }
    }
    else if (argv[i] === '--expect-no-read' && argv[i + 1]) {
      for (const item of String(argv[++i]).split(',')) {
        const trimmed = item.trim();
        if (trimmed) { denyCodeFiles.push(trimmed); expectNoReadFiles.push(trimmed); }
      }
    }
    else if (argv[i] === '--deny-code-dir' && argv[i + 1]) {
      for (const item of String(argv[++i]).split(',')) {
        const trimmed = item.trim().replace(/^\/+/, '');
        if (trimmed) denyCodeDirs.push(trimmed.endsWith('/') ? trimmed : trimmed + '/');
      }
    }
  }
  if (!['root', 'slash', 'sub-root', 'no-utf8', 'missing-pack'].includes(wxPathStyle)) {
    fail(`未知的 --wx-path-style: ${wxPathStyle}`);
  }
  if (!['off', 'auto', 'thin', 'corrupt', 'down', 'png-fail'].includes(remoteMode)) {
    fail(`未知的 --remote: ${remoteMode}`);
  }
  if (!['', 'stub', 'frozen', 'locked'].includes(hostDocument)) {
    fail(`未知的 --host-document: ${hostDocument}（可选 stub / frozen / locked）`);
  }
  if (!['', 'separate'].includes(hostWindowMode)) {
    fail(`未知的 --host-window: ${hostWindowMode}（可选 separate）`);
  }
  if (!['', 'offscreen'].includes(hostCanvasMode)) {
    fail(`未知的 --host-canvas: ${hostCanvasMode}（可选 offscreen）`);
  }
  if (!['auto', 'off', 'ok', 'never', 'delay', 'fail-once'].includes(subpackageMode)) {
    fail(`未知的 --subpackage: ${subpackageMode}（可选 auto / off / ok / never / delay / fail-once）`);
  }
  // 分包声明：直接读发布包里的 game.json，判据与适配层的 hasPackSubpackage 一致。
  const declaredPackSubpackage = (() => {
    try {
      const config = JSON.parse(fs.readFileSync(path.join(root, 'game.json'), 'utf8'));
      const list = config.subpackages || config.subPackages || [];
      return list.some((item) => item && String(item.root || '').replace(/\/$/, '') === 'pack');
    } catch (error) { return false; }
  })();
  // never / delay / fail-once / expect-pack-failure 都是在**声明了分包**的包上才有意义的宿主行为；
  // 在 full/remote 形态的包上跑它们只会得到一条"什么都没发生"的绿用例（假绿比红危险）。
  if (subpackageMode !== 'auto' && subpackageMode !== 'off' && !declaredPackSubpackage) {
    fail(`--subpackage ${subpackageMode} 需要发布包声明 pack 分包（当前 game.json 未声明）：${root}`);
  }
  if (expectPackFailure && !declaredPackSubpackage) {
    fail(`--expect-pack-failure 需要发布包声明 pack 分包（当前 game.json 未声明）：${root}`);
  }
  // fail-once 的第二次尝试由"下一次 pack/ 读取"触发，而默认用例从头到尾只待在标题界面
  // （首屏资源全在主包，一次 pack/ 读取都不发生）→ 永远不会有第二次尝试，用例会假绿。
  if (subpackageMode === 'fail-once' && !driveNewGame) {
    fail('--subpackage fail-once 需要配合 --drive-new-game：重试由"进玩法场景时的 pack/ 读取"触发，' +
      '只跑标题界面的话第二次尝试根本不会发生，这条用例会假绿');
  }
  // **挂载门**：声明了分包、且没被 --subpackage off 关掉时，未挂载就一律读不到 pack/。
  // 必须由宿主侧强制，而不是靠被测代码自觉——否则"忘了 loadSubpackage 也照样全绿"，
  // 而真机上那就是"标题能出、一进玩法全黑"。
  const subpackageGate = subpackageMode !== 'off' && declaredPackSubpackage;
  let packMounted = false;
  let packMountAttempts = 0;
  // 未挂载期间的拒绝次数，分两段记——这两段说明的问题完全不同：
  //   boot 段（启动 Promise settle 之前）：那一刻屏幕上是标题画面，**所有** pack/ 读取都是
  //   "首屏要用的东西"，被门挡住就意味着首屏清单漏列了。这是清单漏列唯一能在本机被抓的位置。
  //   mount 段（启动之后到挂载前）：玩法场景/预加载要用的非首屏资源，被挡是正常的。
  let packDeniedBeforeBoot = 0;
  let packDeniedBeforeMount = 0;
  // `--deny-code-file` 命中次数（按路径），用来证明"这条用例真的把那个文件挡住了"。
  const deniedCodeReads = new Map();
  // `--deny-code-dir` 命中次数（按前缀）。
  const deniedDirReads = new Map();
  let packMountFrame = -1;
  let bootSettledAt = -1;
  // 启动 Promise settle 的那一刻分包挂上了没有。"首屏不等分包"最直接的证据就是它 = false：
  // 标题画面是在分包还**没挂上**的时候画出来的。
  let packMountedAtBootSettle = null;
  if (!fs.existsSync(path.join(root, 'game.js'))) {
    fail(`发布目录缺少 game.js: ${root}`);
  }
  // 远端模式下包内**读不到** pack/：这正是回落要覆盖的场景。显式传了方言则以传入为准。
  // thin 模式**不能**禁用包内读取——它要验证的正是"包内骨架优先、重资源才走远端"。
  if (remoteMode !== 'off' && remoteMode !== 'thin' && remoteMode !== 'png-fail' && !wxPathStyleGiven) wxPathStyle = 'missing-pack';
  if (remoteMode === 'thin' && !remoteRoot) {
    fail('--remote thin 需要 --remote-root <资源包目录>（内含 pack/，即导出的 CDN 目录）');
  }
  // 资源服务根目录：显式 --remote-root 时用它（auto 模式也可以从别处取完整资源包），否则用发布目录。
  const remoteServeRoot = remoteRoot || root;
  if (remoteMode === 'thin' && !fs.existsSync(path.join(remoteServeRoot, 'pack'))) {
    fail(`--remote-root 下没有 pack/: ${remoteServeRoot}`);
  }

  const userRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'p2-wx-user-'));
  const downloadTmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'p2-wx-dl-'));
  let downloadSeq = 0;
  const pendingImageSrcs = new Set();
const stats = { packReads: 0, subpackageLoads: 0, imagesLoaded: 0, imageFailures: 0, imageFailureSamples: [], keyEvents: [], drawOps: 0, frames: 0, storage: new Map(), remoteRequests: 0, remoteAttempts: 0, cacheWrites: 0, fontLoaded: 0, fontMissing: 0, fontBadMagic: 0 };

  // 真 HTTP GET（远端回落必须走真实网络路径，否则"远端可用"没被验证过）。
  function httpGet(url) {
    return new Promise((resolve, reject) => {
      const request = http.get(url, (response) => {
        const chunks = [];
        response.on('data', (chunk) => chunks.push(chunk));
        response.on('end', () => resolve({ statusCode: response.statusCode, body: Buffer.concat(chunks) }));
      });
      request.on('error', reject);
      request.setTimeout(4000, () => { request.destroy(new Error('harness http timeout')); });
    });
  }

  function toArrayBuffer(buffer) {
    return buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);
  }

  // --- 远端模式：起本地静态服务（复用 scripts/pack-server.js，与手工测试同一份实现），
  //     把 remote-assets.json 写进发布目录（用完恢复），让适配层按真实配置走远端回落。
  const remote = { baseUrl: null, server: null, configPath: path.join(root, 'remote-assets.json'), originalConfig: null, hadConfig: false };
  if (remoteMode !== 'off') {
    remote.hadConfig = fs.existsSync(remote.configPath);
    remote.originalConfig = remote.hadConfig ? fs.readFileSync(remote.configPath) : null;
    if (remoteMode === 'down') {
      // 127.0.0.1:1 无人监听 → 连接立即被拒，用来验证"失败要快、且要大声"。
      remote.baseUrl = 'http://127.0.0.1:1/pack/';
    } else {
      const { createPackServer } = require(path.join(__dirname, 'pack-server.js'));
      const corruptPath = 'pack/project.toml';
      remote.server = createPackServer({
        root: remoteServeRoot,
        log: null,
        // 只翻转一个字节、**保持长度不变**：确保拦住它的是校验和，而不是大小检查。
        transform: remoteMode === 'corrupt'
          ? (rel, buffer) => (rel === corruptPath ? Buffer.from(buffer.map((b, i) => (i === 0 ? b ^ 0xff : b))) : buffer)
          : null,
        // 局部 CDN 故障：文本资源照常、PNG 一律 500 —— 贴图必然失败，用来验证
        // "素材拿不到时屏幕必须自证"，而不是静默黑屏（--expect-image-band 用这个模式）。
        failFor: remoteMode === 'png-fail' ? (rel) => (/\.png$/i.test(rel) ? '500 for ' + rel : null) : null
      });
    }
  }

  async function startRemoteServer() {
    if (!remote.server) return;
    const address = await remote.server.listen(0, '127.0.0.1');
    remote.baseUrl = `http://127.0.0.1:${address.port}/pack/`;
  }

  function configureRemote() {
    if (!remote.baseUrl) return;
    fs.writeFileSync(remote.configPath, JSON.stringify({ packBaseUrl: remote.baseUrl, verifyChecksum: true, timeoutMs: 3000 }, null, 2));
  }

  function restoreRemoteConfig() {
    if (remoteMode === 'off') return;
    try {
      if (remote.hadConfig) fs.writeFileSync(remote.configPath, remote.originalConfig);
      else if (fs.existsSync(remote.configPath)) fs.unlinkSync(remote.configPath);
    } catch { /* 清理失败不影响结论，但会在下面断言里暴露 */ }
  }

  // --- 伪造 2D 上下文：记录绘制调用。`fillRect` 会被**真的记到像素上**，
  const noop = () => {};
  //     这样适配层的"像素回读自证"（画一块 #ff00ff 再 getImageData 读回来）是**真验证**，
  //     而不是被一句 "只要有绘制就返回砖红色" 糊弄过去。`clearRect` 按真实语义清空。
  const painted = [];
  // 画到屏幕上的**文本**（fillText）。适配层的看门狗自证全靠它：光有 console.error 还不够，
  // "启动超时"必须真的出现在画布上——否则玩家看到的还是一片黑（--expect-boot-stall 用这个）。
  const drawnTexts = [];
  function parseColor(style) {
    const text = String(style == null ? '' : style).trim();
    let match = /^#([0-9a-f]{6})$/i.exec(text);
    if (match) {
      const value = parseInt(match[1], 16);
      return [(value >> 16) & 255, (value >> 8) & 255, value & 255, 255];
    }
    match = /^#([0-9a-f]{3})$/i.exec(text);
    if (match) {
      const parts = match[1].split('').map((ch) => parseInt(ch + ch, 16));
      return [parts[0], parts[1], parts[2], 255];
    }
    match = /^rgba?\(([^)]+)\)$/i.exec(text);
    if (match) {
      const parts = match[1].split(',').map((part) => Number(part.trim()));
      if (parts.length >= 3 && parts.slice(0, 3).every((value) => !Number.isNaN(value))) {
        return [parts[0] | 0, parts[1] | 0, parts[2] | 0, parts.length > 3 ? Math.round((parts[3] || 0) * 255) : 255];
      }
    }
    return null;
  }
  // 适配层自绘（进度条 / 警示条 / 覆盖层 / 像素自检）会把 `ctx.__p2AdapterDraw` 置真
  // （见 wechat-adapter.js 的 withAdapterDraw）。这些笔数**不计入** stats.drawOps：
  // "每帧绘制数严格递增"这条断言要证明的是**游戏**在画，而适配层的笔数随自身状态增减
  // （分包一开始下载每帧多画几笔，挂载完成后又少画几笔）→ 递减 → 假红。
  // 口径是把适配层的笔**排除**，不是放宽断言：游戏自己不画了，这条依然会红。
  function countDraw(ctx) {
    if (ctx && ctx.__p2AdapterDraw) return;
    stats.drawOps++;
    if (ctx) ctx.__ops++;
  }

  function createRecordingContext() {
    const ctx = {
      canvas: null,
      __ops: 0,
      fillStyle: '#000', strokeStyle: '#000', lineWidth: 1, font: '10px sans-serif',
      textAlign: 'left', textBaseline: 'alphabetic', globalAlpha: 1,
      save: noop, restore: noop, translate: noop, scale: noop, rotate: noop,
      setTransform: noop, transform: noop, resetTransform: noop, clip: noop,
      beginPath: noop, closePath: noop, moveTo: noop, lineTo: noop, arc: noop, rect: noop,
      fill: noop, stroke: noop,
      fillText: (text) => { drawnTexts.push(String(text)); },
      strokeText: (text) => { drawnTexts.push(String(text)); },
      setLineDash: noop,
      createLinearGradient: () => ({ addColorStop: noop }),
      createRadialGradient: () => ({ addColorStop: noop }),
      measureText: (text) => ({ width: String(text).length * 6 }),
      getImageData: (x, y, w, h) => {
        const startX = Math.floor(Number(x) || 0);
        const startY = Math.floor(Number(y) || 0);
        const width = Math.max(1, Math.floor(w) || 1);
        const height = Math.max(1, Math.floor(h) || 1);
        const data = new Uint8ClampedArray(width * height * 4);
        for (let row = 0; row < height; row++) {
          for (let col = 0; col < width; col++) {
            const px = startX + col;
            const py = startY + row;
            let color = null;
            for (let i = painted.length - 1; i >= 0; i--) {
              const rect = painted[i];
              if (px >= rect.x && px < rect.x + rect.w && py >= rect.y && py < rect.y + rect.h) { color = rect.color; break; }
            }
            if (!color) continue;
            const offset = (row * width + col) * 4;
            data[offset] = color[0]; data[offset + 1] = color[1]; data[offset + 2] = color[2]; data[offset + 3] = color[3];
          }
        }
        return { data, width, height };
      }
    };
    ctx.fillRect = function (x, y, w, h) {
      countDraw(ctx);
      const color = parseColor(this && this.fillStyle !== undefined ? this.fillStyle : ctx.fillStyle);
      if (color) {
        painted.push({ x: Number(x) || 0, y: Number(y) || 0, w: Number(w) || 0, h: Number(h) || 0, color });
        if (painted.length > 1024) painted.shift();
      }
    };
    ctx.strokeRect = function () { countDraw(ctx); };
    // 贴图也要落到像素上：运行时的画面自检（web-runtime.js 的 sampleDrawnPixels）是**真的**
    // 采样 canvas 像素来回答"启动了但有没有画出东西"。假 ctx 若不记录 drawImage 的落点，
    // 这条断言就退化成"只有 fillRect 才算数"，`data-p2-drawn` 会凭空变小。
    ctx.drawImage = function (image) {
      countDraw(ctx);
      const rest = Array.prototype.slice.call(arguments, 1).map((value) => Number(value) || 0);
      let dx = 0, dy = 0, dw = 0, dh = 0;
      if (rest.length >= 8) { dx = rest[4]; dy = rest[5]; dw = rest[6]; dh = rest[7]; }
      else if (rest.length >= 4) { dx = rest[0]; dy = rest[1]; dw = rest[2]; dh = rest[3]; }
      else {
        dx = rest[0] || 0; dy = rest[1] || 0;
        dw = Number(image && image.width) || 32; dh = Number(image && image.height) || 32;
      }
      if (dw > 0 && dh > 0) {
        painted.push({ x: dx, y: dy, w: dw, h: dh, color: [200, 120, 60, 255] });
        if (painted.length > 1024) painted.shift();
      }
    };
    // clearRect 在真实 canvas 上会把已画内容清掉：这里照做，否则"覆盖层画完又被清掉"会被测成通过。
    ctx.clearRect = function () { countDraw(ctx); painted.length = 0; };
    return ctx;
  }

  const canvas = {
    width: 0, height: 0,
    getContext(kind) {
      if (kind !== '2d') return null;
      if (!this.__ctx) { this.__ctx = createRecordingContext(); this.__ctx.canvas = this; }
      return this.__ctx;
    },
    loadFont: hostLoadFont
  };
  // `--host-canvas offscreen`：模拟"上屏画布挂在全局、而 wx.createCanvas() 返回的是另一个
  // （离屏）对象"——开发者工具/真机上引导代码先调过一次 createCanvas 就会这样。适配层必须
  // 选择**上屏画布**，否则一切正常却全黑（正是本次要覆盖的故障类）。
  const offscreenCanvas = {
    width: 0, height: 0,
    getContext(kind) {
      if (kind !== '2d') return null;
      if (!this.__ctx) { this.__ctx = createRecordingContext(); this.__ctx.canvas = this; }
      return this.__ctx;
    }
  };

  function readBinary(filePath) {
    const resolved = resolvePath(filePath);
    return resolved ? fs.readFileSync(resolved) : null;
  }
  // 按 `--wx-path-style` 模拟宿主对代码包/分包路径的接受规则：
  //   root    : 只接受根相对路径（pack/x.png）
  //   slash   : 只接受前导斜杠（/pack/x.png）
  //   sub-root: 只接受分包根相对路径（x.png → <root>/pack/x.png）
  //   no-utf8 : 同 root，但对带 encoding 的 readFileSync 抛错（迫使适配层走字节解码）
  function resolveHostPathUnrestricted(filePath) {
    const p = String(filePath == null ? '' : filePath);
    // 平台下发的临时文件（wx.downloadFile 的 tempFilePath）在真机上可以直接读，
    // 不属于代码包路径规则管辖——否则"下载成功却读不回字节"会变成假故障。
    if (downloadTmpDir && p.startsWith(downloadTmpDir)) {
      return fs.existsSync(p) ? p : null;
    }
    if (userRoot && p.startsWith(userRoot)) {
      const rel = p.slice(userRoot.length).replace(/^[\\/]+/, '');
      const abs = path.join(userRoot, rel);
      return fs.existsSync(abs) ? abs : null;
    }
    const rootRelative = !p.startsWith('/') && !p.startsWith('./');
    const rel = p.replace(/^\.\//, '').replace(/^\/+/, '');
    // **挂载门**（与上面的方言分支正交的另一轴）：分包未挂载时 `pack/` 下的文件一律读不到。
    // 这是真机的硬语义——`subpackages` root 目录在挂载前根本不在沙箱里。由宿主侧强制，
    // 才谈得上"首屏必须全在主包"这件事被真的验证过；放在被测代码里自觉就等于没验。
    if (subpackageGate && !packMounted && rel.startsWith('pack/')) {
      packDeniedBeforeMount++;
      if (bootSettledAt < 0) packDeniedBeforeBoot++;
      return null;
    }
    // 宿主不给读的文件（`--deny-code-file`）：**所有**代码包方言一律拒绝，与真机上那串
    // `permission denied` 一模一样（真机对 user-data 给的是 no such file，这里也保留那个区别，
    // 因为"两个来源的报错不同"正是当初判断"代码包这条路根本没通"的线索之一）。
    if (denyCodeFiles.length && denyCodeFiles.includes(rel)) {
      deniedCodeReads.set(rel, (deniedCodeReads.get(rel) || 0) + 1);
      return null;
    }
    // 整个子目录被拒（真机对"未声明的子目录"就是这个行为）。**必须排在分包挂载门之后**：
    // 已声明的分包 root 在挂载后是可读的，这正是它与 `mainpack/` 的区别所在。
    for (const dir of denyCodeDirs) {
      if (rel.startsWith(dir)) {
        deniedDirReads.set(dir, (deniedDirReads.get(dir) || 0) + 1);
        return null;
      }
    }
    // missing-pack：模拟"文件确实没进包"（例如宿主过滤了扩展名）——任何路径形式都读不到，
    // 用于反向验证"读不到资源时 harness 必须失败并打印宿主原始错误"，而不是静默照常通过。
    if (wxPathStyle === 'missing-pack' && rel.startsWith('pack/')) return null;
    if (wxPathStyle === 'slash') {
      if (!p.startsWith('/')) return null;
      const abs = path.join(root, rel);
      return fs.existsSync(abs) ? abs : null;
    }
    if (wxPathStyle === 'sub-root') {
      // 宿主把"已加载的分包"挂成独立根：主包里**没有** pack/ 这份目录，所以带 pack/ 前缀的
      // 路径一律读不到；其余路径先按工程根解析（主包文件），再按分包根解析。
      // 注意：这条规则让 pack/game.js 的"分包根相对"形式会命中主包入口 game.js（同名），
      // 这是真实存在的歧义，适配层因此把该策略排在最后，自检也刻意不探测这个文件。
      if (!rootRelative || rel.startsWith('pack/')) return null;
      const main = path.join(root, rel);
      if (fs.existsSync(main)) return main;
      const inPack = path.join(root, 'pack', rel);
      return fs.existsSync(inPack) ? inPack : null;
    }
    if (!rootRelative) return null;
    const abs = path.join(root, rel);
    return fs.existsSync(abs) ? abs : null;
  }

  // 解析结果的**拒绝原因**（供 FSM 的错误文案区分两种宿主语义；默认重置）。
  let lastDenyReason = null;

  // 真机模型外壳：任何代码包路径的读取都必须落在"会被上传且读得到"的集合内。
  // 只加这一层就覆盖了全部收口点——因为 FSM、`Image.src`、`wx.loadFont` 三条通道最终都走它。
  function resolveHostPath(filePath) {
    lastDenyReason = null;
    const abs = resolveHostPathUnrestricted(filePath);
    if (!abs) return null;
    if (uploadSetBypass) return abs;
    const reason = codePackageDenyReason(abs);
    if (reason) {
      lastDenyReason = reason;
      codeReadDenied += 1;
      return null;
    }
    return abs;
  }

  // 少数场景需要绕过真机模型（例如 harness 自己校验"磁盘上到底有什么"），
  // 让"模型"与"磁盘事实"能分开断言，而不是把两者混成一个结论。
  let uploadSetBypass = false;
  function withoutUploadModel(fn) {
    uploadSetBypass = true;
    try { return fn(); }
    finally { uploadSetBypass = false; }
  }

  // 写入/建目录的路径解析：**不做存在性检查**（目标本来就不存在），
  // 且只有 USER_DATA_PATH 下可写——与真实宿主一致（代码包只读）。
  function resolveWritePath(filePath) {
    const p = String(filePath == null ? '' : filePath);
    if (downloadTmpDir && p.startsWith(downloadTmpDir)) return p;
    if (userRoot && p.startsWith(userRoot)) {
      return path.join(userRoot, p.slice(userRoot.length).replace(/^[\\/]+/, ''));
    }
    return null;
  }

  // 脚本自身的磁盘读取（校验基准）：不受 --wx-path-style 限制——受限的是"被测代码能读到什么"。
  function resolvePath(filePath) {
    const p = String(filePath == null ? '' : filePath);
    if (userRoot && p.startsWith(userRoot)) {
      const rel = p.slice(userRoot.length).replace(/^[\\/]+/, '');
      const abs = path.join(userRoot, rel);
      return fs.existsSync(abs) ? abs : null;
    }
    const rel = p.replace(/^\.\//, '').replace(/^\/+/, '');
    const direct = path.join(root, rel);
    if (fs.existsSync(direct)) return direct;
    const inPack = path.join(root, 'pack', rel);
    return fs.existsSync(inPack) ? inPack : null;
  }

  function pngSize(buffer) {
    if (!buffer || buffer.length < 24) return null;
    const isPng = buffer[0] === 0x89 && buffer[1] === 0x50 && buffer[2] === 0x4e && buffer[3] === 0x47;
    if (!isPng) return null;
    return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
  }

  // 在 pack/ 里找一个二进制资源（PNG），用于验证 arrayBuffer() 是否按真实字节读取。
  // 找一张 PNG 作为"二进制读取必须逐字节一致"的探针。
  // `searchRoot` 默认是发布目录：瘦首包里本地没有 PNG，这时要到**远端资源包**里挑，
  // 才能验证"下载回来的字节和 CDN 上那份完全一致"。
  function findPackBinary(searchRoot, baseForRelative) {
    const base = searchRoot || root;
    const packRoot = path.join(base, 'pack');
    const stack = [packRoot];
    while (stack.length) {
      const dir = stack.pop();
      let entries = [];
      try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { continue; }
      for (const entry of entries) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) { stack.push(full); continue; }
        if (/\.png$/i.test(entry.name)) {
          return path.relative(baseForRelative || root, full).replace(/\\/g, '/');
        }
      }
    }
    return null;
  }

  // "运行时读了一个 pack 素材"——满足任一即算：请求路径带 pack/（含缓存路径），
  // 或解析后落在发布包的 pack/ 目录里（subpackage-relative 方言会把 pack/ 前缀去掉，
  // 单看请求路径会漏计）。
  function isPackRequest(filePath, abs) {
    const p = String(filePath == null ? '' : filePath).replace(/\\/g, '/');
    const rel = p.replace(/^\.?\//, '');
    if (rel.startsWith('pack/') || rel.includes('/pack/')) return true;
    if (abs && abs.startsWith(path.join(root, 'pack') + path.sep)) return true;
    if (abs && userRoot && abs.startsWith(path.join(userRoot, 'pack') + path.sep)) return true;
    return false;
  }

  // ---------------------------------------------------------------- 真机代码包模型
  // **本机假绿的总根源**：以前的 `resolveHostPath` 直接从磁盘读，于是"真机上根本读不到的文件"
  // 在本机全部读得到——91 个 `.lua`/`.toml` 因为后缀不在微信上传白名单里而从未被上传，
  // 本机却一路全绿，直到真机启动失败（`pack/project.toml HTTP 404`）。
  //
  // 现在**默认常开**这套模型（不是可选开关）：先把磁盘上的产物换算成"真机上会被上传且读得到的
  // 文件集合"，此后所有代码包路径的读取都必须落在这个集合内。判据全部来自
  // scripts/code-package-rules.js（与构建期同一份规则），并额外减去产物里 `project.config.json`
  // 的 `packOptions.ignore`（配置即真相：被 ignore 的文件不会进包）。
  let uploadedSet = null;
  let ignoredMatchers = null;

  function loadPackOptionsIgnore() {
    if (ignoredMatchers) return ignoredMatchers;
    ignoredMatchers = [];
    const configPath = path.join(root, 'project.config.json');
    if (!fs.existsSync(configPath)) return ignoredMatchers;
    let config = null;
    try { config = JSON.parse(fs.readFileSync(configPath, 'utf8')); } catch (error) { return ignoredMatchers; }
    const list = (config && config.packOptions && config.packOptions.ignore) || [];
    for (const item of list) {
      if (!item || item.value === undefined) continue;
      const value = String(item.value).replace(/^\/+/, '').replace(/\\/g, '/');
      if (item.type === 'file') ignoredMatchers.push((rel) => rel === value);
      else if (item.type === 'folder') ignoredMatchers.push((rel) => rel === value || rel.startsWith(value + '/'));
      else if (item.type === 'suffix') ignoredMatchers.push((rel) => rel.toLowerCase().endsWith(value.toLowerCase()));
      else if (item.type === 'prefix') ignoredMatchers.push((rel) => rel.startsWith(value));
      // regexp / glob 不建模：本仓库的产物只用 file/folder/suffix/prefix，
      // 出现别的类型时下面的输出行会报"未建模的 ignore 规则"，不静默。
      else ignoredMatchers.push(() => false);
    }
    return ignoredMatchers;
  }

  function isIgnoredPath(rel) {
    return loadPackOptionsIgnore().some((match) => match(rel));
  }

  // 会被上传且上传后**读得到**的代码包文件（精确大小写，`/` 分隔）。
  // 注意 key 用精确大小写的相对路径而不是文件系统查询：真机（iOS/Android）区分大小写，
  // 而 Windows 上 `fs.existsSync` 不区分——用集合查询才能让本机拥有真机的语义。
  function loadUploadedSet() {
    if (uploadedSet) return uploadedSet;
    uploadedSet = new Set();
    const walk = (dir) => {
      let entries = [];
      try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch (error) { return; }
      for (const entry of entries) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) { walk(full); continue; }
        if (!entry.isFile()) continue;
        const rel = path.relative(root, full).split(path.sep).join('/');
        if (isIgnoredPath(rel)) continue;
        if (rules.isReadableInPackage(rel)) uploadedSet.add(rel);
      }
    };
    walk(root);
    return uploadedSet;
  }

  function uploadedStats() {
    return { uploaded: loadUploadedSet().size, unuploadable: unuploadableHits.size, deniedReads: codeReadDenied };
  }

  // 逻辑路径 → 产物里实际的宿主文件名（查产物根的 code-package.json）。
  // 只有"核实磁盘事实"的断言才该用它；经过真机模型的那条读取路径由适配层自己查表。
  let hostNameTable = null;
  function hostNameOfLogical(logical) {
    if (!hostNameTable) {
      hostNameTable = Object.create(null);
      const mapPath = path.join(root, 'code-package.json');
      if (fs.existsSync(mapPath)) {
        try {
          const parsed = JSON.parse(fs.readFileSync(mapPath, 'utf8'));
          for (const key of Object.keys(parsed.renamed || {})) hostNameTable[key] = String(parsed.renamed[key]);
        } catch (error) { /* 读不出来就按逻辑名处理，下面的断言会因此报出来 */ }
      }
    }
    return hostNameTable[logical] || logical;
  }

  const unuploadableHits = new Set();
  let codeReadDenied = 0;

  // 这个绝对路径在真机上读得到吗？（代码包路径：不在上传集合内就是读不到）
  // 返回 null = 允许读；返回 'permission denied' = 文件在项目里但不在包的可读集合内
  // （真机对这种情况给的就是 permission denied，与"磁盘上也没有"的 not found 分开——
  //  两种报错的差别正是当初判断"代码包这条路根本没通"的线索）。
  function codePackageDenyReason(abs) {
    const rel = path.relative(root, abs).split(path.sep).join('/');
    if (rel.startsWith('..')) return null; // 不在产物里，交给调用方
    const uploaded = loadUploadedSet();
    if (uploaded.has(rel)) return null;
    if (fs.existsSync(abs)) {
      if (rules.isWhitelistedExtension(rel) && rules.isCompiledName(rel)) return 'compiled';
      if (isIgnoredPath(rel)) return 'ignored';
      unuploadableHits.add(rel);
      return 'permission denied';
    }
    return null; // 磁盘上也没有 → 让既有的 not found 语义处理
  }

  // ---------------------------------------------------------------- 字体
  // `wx.loadFont` / `canvas.loadFont` 是**由宿主加载**的接口：以前 harness 直接返回一个常量，
  // 完全不看路径——于是"字体文件读不到"在本机永远绿（真机上只是中文变系统字体，很难发现）。
  // 现在真去读一遍并校验 TTF 魔数，读不到就返回 null（适配层会据此降级并打日志）。
  function looksLikeTtf(buffer) {
    if (!buffer || buffer.length < 4) return false;
    const magic = buffer.readUInt32BE(0);
    return magic === 0x00010000 || magic === 0x74727565 /* true */ || magic === 0x4f54544f /* OTTO */ || magic === 0x74746366 /* ttcf */;
  }

  function hostLoadFont(filePath) {
    const abs = resolveHostPath(filePath);
    if (!abs) { stats.fontMissing++; return null; }
    try {
      if (!looksLikeTtf(fs.readFileSync(abs))) { stats.fontBadMagic++; return null; }
    } catch (error) { stats.fontMissing++; return null; }
    stats.fontLoaded++;
    return 'WeChatHarnessFont';
  }

  // 适配层对 FSM 的用法是 `typeof fs.x === 'function'` 守卫 + 静默降级。harness 只实现它真正
  // 用到的那几个；一旦适配层将来去探一个没实现的方法，本机会"看起来支持、实际什么都不做"，
  // 而这正是最危险的一类假绿。这里把**未实现的名字记下来**，由断言报出来——
  // 保持返回 undefined（不能返回假函数，那会把适配层的守卫骗过去）。
  const fsmUnknownProbes = new Set();
  const FSM_IMPLEMENTED = new Set(['readFileSync', 'writeFileSync', 'mkdirSync', 'unlinkSync', 'readFile']);

  function recordFsmProbes(target) {
    return new Proxy(target, {
      get(obj, name) {
        if (typeof name === 'string' && !FSM_IMPLEMENTED.has(name) && !(name in obj) && name !== 'then') {
          fsmUnknownProbes.add(name);
        }
        return obj[name];
      }
    });
  }

  const rafQueue = [];
  const wx = {
    env: { USER_DATA_PATH: userRoot },
    getSystemInfoSync: () => ({ windowWidth: 375, windowHeight: 667, pixelRatio: 1, platform: 'harness', system: 'node' }),
    createCanvas: () => (hostCanvasMode === 'offscreen' ? offscreenCanvas : canvas),
    createImage: () => {
      const image = { onload: null, onerror: null, width: 0, height: 0, _src: '' };
      // `configurable: true`：真机上 wx.createImage() 返回的是 JS 层对象，适配层可以给它加一个
      // own property 来接管 src（这也是社区适配器的通行做法）。真机若不允许接管，适配层必须
      // **响亮失败**而不是静默丢贴图——用 --image-src-frozen 专门验证那条路径。
      const srcDescriptor = {
        configurable: !frozenImageSrc,
        get() { return image._src; },
        set(value) {
          image._src = String(value);
          pendingImageSrcs.add(image._src);
          // **必须走宿主受限读取**（resolveHostPath），不能直接从磁盘捞：
          // 真机上 Image.src 只认代码包/用户目录里真实存在的文件，包内没有就会 onerror。
          // 早期这里用脚本级 resolvePath 直接从发布目录读文件，于是"包内没有的贴图"
          // 在无头测试里照样成功，把"瘦首包贴图全丢"这种真问题掩盖掉了（实测）。
          const abs = resolveHostPath(value);
          const buffer = abs ? fs.readFileSync(abs) : null;
          // 宿主收下 src 却永不回调（--image-src-silent）：src 确实进了 Image，但 onload/onerror
          // 一个都不来。此时 pending 会一直挂着 —— 这正是"启动看门狗"唯一能被验证的形态，
          // 也是弱网/CDN 挂起在真机上的样子（卡住但没有任何异常）。
          if (silentImageSrc) return;
          setTimeout(() => {
            pendingImageSrcs.delete(image._src);
            if (!buffer) { stats.imageFailures++; if (stats.imageFailureSamples.length < 4) stats.imageFailureSamples.push(String(value)); if (image.onerror) image.onerror({ errMsg: 'read fail: ' + value }); return; }
            const size = pngSize(buffer);
            if (size) { image.width = size.width; image.height = size.height; }
            else { image.width = 32; image.height = 32; }
            stats.imagesLoaded++;
            if (image.onload) image.onload();
          }, 0);
        }
      };
      if (imageCallbackBridge) {
        // 基础库 2.32.3 的 `class k$ { constructor(e){ e.onload = ()=>{ this.__onload && this.__onload() } }
        //   set src(v){...} set onload(v){ this.__onload = v } get onload(){ return this.__onload } }`
        // 1:1 复现：回调存在包装对象的**私有字段**里，真实 image 的回调在构造时就指向它。
        // `image.onload = fn` 若被 own property 接管，宿主的 setter 就不会执行 → 桥断。
        const real = image;
        const proto = Object.create(Object.prototype, {
          src: srcDescriptor,
          onload: { configurable: true, enumerable: true, get() { return this.__onload; }, set(fn) { this.__onload = fn; } },
          onerror: { configurable: true, enumerable: true, get() { return this.__onerror; }, set(fn) { this.__onerror = fn; } },
          width: { configurable: true, enumerable: true, get() { return real.width; } },
          height: { configurable: true, enumerable: true, get() { return real.height; } }
        });
        const wrapper = Object.create(proto);
        real.onload = () => { if (typeof wrapper.__onload === 'function') wrapper.__onload(); };
        real.onerror = () => { if (typeof wrapper.__onerror === 'function') wrapper.__onerror(); };
        return wrapper;
      }
      if (prototypeImageSrc) {
        // 微信基础库（2.25.3 实测）的形态：`class Image { setSrc(v){…} getSrc(){…} }`，
        // 对外的 `src` 是**原型上的** class 访问器，实例上**没有** own 的 `src`
        // —— `Object.getOwnPropertyDescriptor(image,'src')` 返回 undefined。
        // 只看 own 描述符的适配层会退化成写 `image._src`（基础库是在 setSrc 里发起加载的），
        // 宿主于是永远不开始加载：贴图 Promise 不 settle → 启动静默卡死、屏幕全黑、0 error。
        // 该形态由 --image-src-prototype 1:1 复现。
        Object.setPrototypeOf(image, Object.create(Object.prototype, { src: srcDescriptor }));
      } else {
        Object.defineProperty(image, 'src', srcDescriptor);
      }
      return image;
    },
    getFileSystemManager: () => recordFsmProbes({
      readFileSync(filePath, encoding) {
        const abs = resolveHostPath(filePath);
        if (!abs) throw new Error('readFileSync:fail ' + (lastDenyReason ? 'permission denied, open ' : 'no such file or directory ') + filePath);
        if (encoding && wxPathStyle === 'no-utf8') {
          throw new Error('readFileSync:fail unsupported encoding ' + encoding);
        }
        if (isPackRequest(filePath, abs)) stats.packReads++;
        if (encoding) return fs.readFileSync(abs, 'utf8');
        return fs.readFileSync(abs);
      },
      writeFileSync(filePath, data) {
        const abs = resolveWritePath(filePath);
        if (!abs) throw new Error('writeFileSync:fail not writable (代码包只读) ' + filePath);
        const buffer = data instanceof ArrayBuffer ? Buffer.from(new Uint8Array(data)) : Buffer.from(data);
        fs.writeFileSync(abs, buffer);
        stats.cacheWrites++;
      },
      mkdirSync(dirPath) {
        const abs = resolveWritePath(dirPath);
        if (!abs) throw new Error('mkdirSync:fail not writable (代码包只读) ' + dirPath);
        fs.mkdirSync(abs, { recursive: true });
      },
      unlinkSync(filePath) {
        const abs = resolveWritePath(filePath);
        if (abs && fs.existsSync(abs)) fs.unlinkSync(abs);
      },
      readFile(options) {
        const { filePath, encoding, success, fail: onFail } = options || {};
        try {
          const abs = resolveHostPath(filePath);
          if (!abs) throw new Error('readFileSync:fail ' + (lastDenyReason ? 'permission denied, open ' : 'no such file or directory ') + filePath);
          if (isPackRequest(filePath, abs)) stats.packReads++;
          const data = encoding ? fs.readFileSync(abs, 'utf8') : fs.readFileSync(abs).buffer;
          if (success) success({ data });
        } catch (error) {
          if (onFail) onFail({ errMsg: String(error && error.message || error) });
        }
      }
    }),
    // 远端下载：**真发 HTTP 请求**打到本地服务（不是打桩返回假字节），
    // 否则"远端回落可用"就是个没有验证过的声明。
    downloadFile: (options) => {
      stats.remoteAttempts++;
      const task = { aborted: false, abort() { this.aborted = true; }, onProgressUpdate: noop };
      httpGet(options.url).then((result) => {
        if (task.aborted) return;
        if (result.statusCode !== 200) {
          if (options.fail) options.fail({ errMsg: 'downloadFile:fail HTTP ' + result.statusCode });
          return;
        }
        let target = options.filePath || null;
        try {
          if (!target) {
            target = path.join(downloadTmpDir, 'p2-dl-' + (downloadSeq++) + path.extname(String(options.url).split('?')[0]));
          } else {
            const abs = resolveWritePath(target);
            if (!abs) throw new Error('filePath 不在 USER_DATA_PATH 下（真机会拒绝）: ' + target);
            fs.mkdirSync(path.dirname(abs), { recursive: true });
            target = abs;
          }
          fs.writeFileSync(target, result.body);
        } catch (error) {
          if (options.fail) options.fail({ errMsg: 'downloadFile:fail write ' + String(error && error.message || error) });
          return;
        }
        stats.remoteRequests++;
        if (options.success) options.success({ statusCode: 200, tempFilePath: target, filePath: target });
      }).catch((error) => {
        if (options.fail) options.fail({ errMsg: 'downloadFile:fail ' + String(error && error.message || error) });
      });
      return task;
    },
    request: (options) => {
      stats.remoteAttempts++;
      const task = { aborted: false, abort() { this.aborted = true; } };
      httpGet(options.url).then((result) => {
        if (task.aborted) return;
        if (result.statusCode !== 200) {
          if (options.fail) options.fail({ errMsg: 'request:fail HTTP ' + result.statusCode });
          return;
        }
        stats.remoteRequests++;
        const data = options.responseType === 'arraybuffer' ? toArrayBuffer(result.body) : result.body.toString('utf8');
        if (options.success) options.success({ statusCode: 200, data });
      }).catch((error) => {
        if (options.fail) options.fail({ errMsg: 'request:fail ' + String(error && error.message || error) });
      });
      return task;
    },
    loadSubpackage: (options) => {
      stats.subpackageLoads++;
      const attempt = ++packMountAttempts;
      const task = {
        aborted: false,
        abort() { this.aborted = true; },
        // 适配层的 attachPackProgress 会**同步**注册回调（就在 loadSubpackage 返回之后），
        // 所以这里只存起来、由帧循环按需投递。
        onProgressUpdate(cb) { this.__progress = typeof cb === 'function' ? cb : null; }
      };
      const pending = { task, options, atFrame: 0, progress: 10 };
      // never：永不挂载。fail-once：首次失败、冷却后重试成功（验证失败不会变成永久瘫痪）。
      if (subpackageMode === 'never' || (subpackageMode === 'fail-once' && attempt === 1)) {
        setTimeout(() => failPack(pending, 'mock(' + subpackageMode + ')'), 0);
        return task;
      }
      // delay：交给帧循环在第 N 帧挂载（见 pumpSubpackage）。
      if (subpackageMode === 'delay') {
        pending.atFrame = subpackageDelayFrames;
        pendingMounts.push(pending);
        return task;
      }
      // auto/ok：像真机一样"很快但不同步"地挂上。
      setTimeout(() => mountPack(pending), 0);
      return task;
    },
    loadFont: hostLoadFont,
    requestAnimationFrame: (callback) => { rafQueue.push(callback); return rafQueue.length; },
    cancelAnimationFrame: (id) => { if (id > 0) rafQueue[id - 1] = null; },
    onTouchStart: noop, onTouchMove: noop, onTouchEnd: noop, onTouchCancel: noop,
    onKeyDown: noop, onKeyUp: noop, onShow: noop, onHide: noop, onMemoryWarning: noop,
    getStorageSync: (key) => (stats.storage.has(String(key)) ? stats.storage.get(String(key)) : ''),
    setStorageSync: (key, value) => { stats.storage.set(String(key), value); },
    removeStorageSync: (key) => { stats.storage.delete(String(key)); },
    clearStorageSync: () => { stats.storage.clear(); }
  };

  // 触摸/键盘回调要能被测试驱动，这里覆写成可触发的注册器。
  const inputHandlers = {};
  ['onTouchStart', 'onTouchMove', 'onTouchEnd', 'onTouchCancel', 'onKeyDown', 'onKeyUp', 'onShow', 'onHide']
    .forEach((name) => {
      wx[name] = (handler) => { inputHandlers[name] = handler; };
    });

  // 分包挂载：宿主侧唯一能"让它成功/失败/晚一点"的地方。三种发布形态里只有 subpackage
  // 需要它；full/remote 形态的包不会声明分包，适配层也就不会调用它（那条反向断言在下面）。
  const pendingMounts = [];
  let packEntryExports = null;
  let packEntryError = null;

  function mountPack(pending) {
    if (pending.task.aborted) return;
    packMounted = true;
    packMountFrame = stats.frames;
    // 真机在挂载后会**执行** root 下的 game.js 入口。这里真的 require 一次：只检查"文件存在"
    // 测不出入口里写了非法语句，而那份文件是发布脚本生成的——模板漂移只能在这一步暴露。
    try {
      packEntryExports = require(path.join(root, 'pack', 'game.js'));
      packEntryError = null;
    } catch (error) {
      packEntryError = String((error && error.message) || error);
    }
    if (pending.options && pending.options.success) pending.options.success({});
  }

  function failPack(pending, reason) {
    if (pending.task.aborted) return;
    if (pending.options && pending.options.fail) pending.options.fail({ errMsg: 'loadSubpackage:fail ' + reason });
  }

  // delay 模式由帧循环驱动：这既是"首屏不等分包"的证据（前 N 帧里 pack/ 读不到，标题照常出），
  // 也是进度条唯一能被观察到的窗口（其余模式挂载得太快，一帧都画不到就结束了）。
  function pumpSubpackage() {
    if (!pendingMounts.length) return;
    const still = [];
    for (const pending of pendingMounts) {
      if (pending.task.aborted) continue;
      // 进度回调：真机在下载过程中持续回调，适配层的进度条靠它。没有这一步，
      // "进度条真的消费了 onProgressUpdate"这条断言就无从验证。
      if (pending.task.__progress) {
        pending.task.__progress({ progress: pending.progress, totalBytesWritten: pending.progress, totalBytesExpectedToWrite: 100 });
      }
      pending.progress = Math.min(100, pending.progress + 25);
      if (stats.frames >= pending.atFrame) mountPack(pending);
      else still.push(pending);
    }
    pendingMounts.length = 0;
    Array.prototype.push.apply(pendingMounts, still);
  }

  globalThis.wx = wx;
  if (debugOverlay) stats.storage.set('p2_debug_overlay', '1');
  // 看门狗超时走存储注入（适配层读 p2_boot_timeout_ms）：默认 10s，卡死用例压到 1s 左右，
  // 否则"验证看门狗"本身要等 10 秒，进不了质量门。
  if (bootTimeoutMsOverride > 0) stats.storage.set('p2_boot_timeout_ms', bootTimeoutMsOverride);
  // fail-once 要验的是"失败之后能自愈"，而适配层的重试冷却是 10 秒（自动化等不起）。
  // 这里把冷却压到 0（与看门狗超时同一套注入手法），让重试在同一个用例里真的发生。
  if (subpackageMode === 'fail-once') stats.storage.set('p2_pack_retry_cooldown_ms', '0');
  // 上屏画布挂在全局（真机/开发者工具就是这样）：`--host-canvas offscreen` 时它与
  // wx.createCanvas() 的返回值**不是同一个对象**，适配层必须挑全局这个上屏画布。
  if (hostCanvasMode === 'offscreen') globalThis.GameGlobal = { canvas };
  const originalError = console.error;
  const consoleErrors = [];
  console.error = (...args) => { consoleErrors.push(args.map(String).join(' ')); };
  // 适配层用 console.warn 报告"宿主占用了全局/无法改写 Image.src"这类诊断，必须一起收上来
  // （否则"静默降级"和"处理过了"在日志里长得一样）。这个进程是一次性的，不还原 warn。
  const consoleWarns = [];
  console.warn = (...args) => { consoleWarns.push(args.map(String).join(' ')); };
  // 适配层的自证日志走 console.log，这里**旁路复制**一份（同时照常打印）：
  // 既能在断言里核对"真机上能复制现场"，又不改变 harness 现有的输出格式。
  const originalLog = console.log;
  const consoleLogs = [];
  console.log = (...args) => { consoleLogs.push(args.map(String).join(' ')); originalLog(...args); };

  // 宿主自带残缺全局（微信开发者工具的壳）必须在 require(game.js) **之前**装好：
  // 适配层在模块加载时就会安装自己的 shim。
  if (hostDocument || hostWindowMode) {
    if (hostWindowMode === 'separate') {
      hostStubs.window = { __hostWindow: true };
      Object.defineProperty(globalThis, 'window', { value: hostStubs.window, writable: false, configurable: false });
    }
    if (hostDocument === 'locked' || hostDocument === 'stub' || hostDocument === 'frozen') {
      // `locked` = 开发者工具的真实形态（由实测日志反推）：`document` 全局**不可写/不可配置**，
      // 对象**不可扩展**，`Object.keys` 只有 `location`，另有三个不可枚举自有属性
      // （documentElement/querySelector/getElementById）且都不是函数——于是只有这三个还能被
      // 重定义，`body`/`createElement`/`fonts` 一律补不动。适配层必须在这种情况下**照样把游戏
      // 跑起来**（只要能补上 getElementById 就够），而不是在模块加载期抛错——抛出去会让
      // game.js 整个中止、`__RustLuaWechat` 都不存在，屏幕上只剩黑屏（真机实测踩过）。
      const locked = { location: { href: 'wxfile://harness/page.html' } };
      for (const name of ['documentElement', 'querySelector', 'getElementById']) {
        Object.defineProperty(locked, name, {
          value: name === 'documentElement' ? null : undefined,
          configurable: true,
          enumerable: false,
          writable: true
        });
      }
      if (hostDocument === 'locked') Object.preventExtensions(locked);
      // 非 locked 的两个形态额外给一个**看着像 DOM、其实不能用**的 body（有 appendChild、
      // 没有 setAttribute）：它会让 data-p2-boot / data-p2-drawn 全都写不进去，
      // 也就是让**诊断自己撒谎**（画面画了却报 drawn=0）。
      if (hostDocument !== 'locked') locked.body = { appendChild: () => {}, style: {} };
      hostStubs.document = hostDocument === 'frozen' ? Object.freeze(locked) : locked;
      Object.defineProperty(globalThis, 'document', { value: hostStubs.document, writable: false, configurable: false });
    }
  }

  // 远端服务必须先起来、配置必须先落地，然后才允许 require(game.js) 触发启动。
  await startRemoteServer();
  configureRemote();
  const remoteStartedAt = Date.now();

  try {
    require(path.join(root, 'game.js'));
  } catch (error) {
    const loadMessage = String((error && error.message) || error);
    // 宿主把 document 冻结成一个补不齐的残缺对象时，适配层在**模块加载期**就应当失败
    // （安装 shim 是模块级动作）。这就是这个用例要的"响亮失败"，不是崩溃。
    if (hostDocument === 'frozen' && /被宿主占用且补不齐/.test(loadMessage)) {
      const loud = consoleWarns.some((line) => /宿主已占用全局/.test(String(line)));
      if (!loud) {
        restoreRemoteConfig();
        fail('适配层响亮失败了，但没有报告宿主占用了全局（诊断不完整）');
      }
      restoreRemoteConfig();
      console.log(`wechat_minigame_harness_negative_ok case=host-document-frozen boot=load-failed loud=yes frames=0 elapsed_ms=${Date.now() - remoteStartedAt}`);
      if (remote.server) await remote.server.close();
      return 0;
    }
    console.error = originalError;
    // 只报消息与首个栈帧：发布包里的 fengari-web.js 是压缩过的单行文件，
    // 打印完整 stack 会把几 MB 源码灌进日志。
    const firstFrame = String((error && error.stack) || '').split('\n')[1] || '';
    restoreRemoteConfig();
    fail(`加载 game.js 失败: ${loadMessage.slice(0, 600)}\n  ${firstFrame.trim()}`);
  }
  const bootPromise = globalThis.__RustLuaWechatBoot;
  if (!bootPromise || typeof bootPromise.then !== 'function') {
    console.error = originalError;
    restoreRemoteConfig();
    fail('game.js 未暴露启动 Promise（__RustLuaWechatBoot）');
  }

  const adapter = globalThis.__RustLuaWechat;
  const drawnEvents = [];

  // 启动 Promise 在失败时会 reject（适配层故意如此，便于宿主上报）。反向用例要的就是这个失败，
  // 所以这里先把它"接住"记录成 bootError，再在分支里各自断言——否则会先掉进 .catch 直接退出，
  // 反向用例的断言根本没机会跑（实测如此）。
  const bootOutcome = { error: null };
  const bootSettled = bootPromise.then(
    () => { },
    (error) => { bootOutcome.error = error; }
  ).then(() => { bootSettledAt = Date.now(); packMountedAtBootSettle = packMounted; });

  // **启动必须有截止时间**：启动 Promise 既不 resolve 也不 reject 时（例如某张贴图永远
  // 不触发 onload/onerror），早期实现会一直等下去——那既不是"通过"也不是"失败"，
  // 而是把整条验证链挂死（实测 600s 超时）。这里改成明确失败，并点名还没完成的贴图。
  const bootDeadlineMs = bootDeadlineOverride > 0 ? bootDeadlineOverride : Number(process.env.P2_BOOT_TIMEOUT_MS || 30000);
  const bootWinner = await Promise.race([
    bootSettled.then(() => 'settled'),
    new Promise((resolve) => { setTimeout(() => resolve('timeout'), bootDeadlineMs); })
  ]);
  if (bootWinner === 'timeout') {
    const pending = Array.from(pendingImageSrcs).slice(0, 6);
    console.error = originalError;
    const logs = consoleErrors.slice(0, 12);
    restoreRemoteConfig();
    // --expect-boot-stall：卡死是**预期**行为（宿主收下 src 却永不回调）。这一刻要验的不是
    // "没卡死"，而是"卡死了有人管"：看门狗必须报警、而且必须**画到屏幕上**——
    // 真机/开发者工具里没有控制台，只 console.error 等于还是黑屏（这正是本次黑屏的教训）。
    if (expectBootStall) {
      const stallLog = logs.find((line) => line.includes('启动超时'));
      if (!stallLog) {
        fail(`--expect-boot-stall 下适配层的启动看门狗没有报警（${bootDeadlineMs}ms 内没有看到"启动超时"）` +
          `；适配层诊断 ${logs.length} 条:\n    ${logs.length ? logs.join('\n    ') : '（无）'}`);
      }
      const stallText = drawnTexts.find((text) => text.includes('启动超时'));
      if (!stallText) {
        fail(`--expect-boot-stall 下虽然报了警，但没有把"启动超时"画到画布上——玩家看到的仍是一片黑` +
          `；画到屏幕上的文本 ${drawnTexts.length} 条: ${drawnTexts.slice(0, 8).join(' | ') || '（无）'}`);
      }
      const stallDiag = adapter.diag ? adapter.diag() : {};
      console.log(`wechat_minigame_harness_negative_ok case=boot-stall boot=${globalThis.__RustLuaWechat && globalThis.__RustLuaWechat.getAttribute('data-p2-boot')}` +
        ` pending_images=${pendingImageSrcs.size} watchdog_ms=${bootTimeoutMsOverride || 'default'} on_screen=yes` +
        ` image_src_takeover=${stallDiag.image_src_takeover || 'n/a'} frames=${stats.frames} elapsed_ms=${Date.now() - remoteStartedAt}`);
      return 0;
    }
    fail(`启动 Promise 在 ${bootDeadlineMs}ms 内没有 settle（boot=${globalThis.__RustLuaWechat && globalThis.__RustLuaWechat.getAttribute('data-p2-boot')}）` +
      `；未完成加载的贴图 ${pendingImageSrcs.size} 张: ${pending.length ? pending.join(' | ') : '（无）'}` +
      `；统计 pack_reads=${stats.packReads} subpackage_loads=${stats.subpackageLoads} image_failures=${stats.imageFailures} remote_attempts=${stats.remoteAttempts}` +
      `；适配层诊断 ${logs.length} 条:\n    ${logs.length ? logs.join('\n    ') : '（无）'}`);
  }

  // 键盘事件桥接自检（不依赖游戏语义）：注册监听后模拟触摸/键盘，确认事件真的到达运行时宿主。
  const windowTarget = globalThis.window;
  windowTarget.addEventListener('keydown', (event) => { drawnEvents.push(String(event.key)); });

  return bootSettled.then(async () => {
    // 触摸：左半屏推动虚拟摇杆 → 应当派发 'w'
    if (inputHandlers.onTouchStart) {
      inputHandlers.onTouchStart({ touches: [{ identifier: 1, clientX: 60, clientY: 300 }], changedTouches: [{ identifier: 1, clientX: 60, clientY: 300 }] });
      inputHandlers.onTouchMove({ touches: [{ identifier: 1, clientX: 60, clientY: 250 }], changedTouches: [{ identifier: 1, clientX: 60, clientY: 250 }] });
      inputHandlers.onTouchEnd({ touches: [], changedTouches: [{ identifier: 1, clientX: 60, clientY: 250 }] });
    }
    const stickKeys = drawnEvents.slice();

    // 键盘（微信 PC 端）：ArrowUp → 'ArrowUp'
    if (inputHandlers.onKeyDown) inputHandlers.onKeyDown({ key: 'ArrowUp', code: 'ArrowUp' });

    // 泵帧：运行时每帧会再次 requestAnimationFrame。逐帧记录绘制调用数，
    // 这样"只跑了第一帧就卡住"不会被误判为通过。
    // `--no-frames`：一帧都不泵（模拟"帧循环压根没起来"），用来断言覆盖层/自证图案
    // **不依赖帧循环**——真机上最常见的一种黑屏就是"启动流程走完了但一帧都没画"。
    const perFrameDraws = [];
    // `--drive-move N` 的移动驱动要在世界稳定之后单独占 N+1 帧（松键后还要一帧让自报值刷新）。
    // 这些帧从**总预算**里扣：`stats.frames !== frames` 这条断言仍然表示"帧预算跑满了"。
    const moveBudget = driveMove > 0 ? driveMove + 1 : 0;
    for (let i = 0; i < (noFrames ? 0 : Math.max(0, frames - moveBudget)); i++) {
      // 玩家真实路径：按一下确认（空格）触发「开始新游戏」。按下一帧、松开下一帧，
      // 这样 `just_pressed` 只在一帧内为真（与引擎的 just_pressed 语义一致）。
      if (driveNewGame) {
        if (i === driveFrame && inputHandlers.onKeyDown) inputHandlers.onKeyDown({ key: ' ', code: 'Space' });
        if (i === driveFrame + 1 && inputHandlers.onKeyUp) inputHandlers.onKeyUp({ key: ' ', code: 'Space' });
      }
      const callback = rafQueue.shift();
      if (!callback) break;
      callback(Date.now());
      stats.frames++;
      // 分包挂载/进度由帧循环驱动（只对 --subpackage delay 有效）：必须在 afterFrame 之后，
      // 这样"这一帧还没挂载"是真的没挂载，`denied_before_mount` 才有意义。
      pumpSubpackage();
      perFrameDraws.push(stats.drawOps);
      await new Promise((resolve) => setTimeout(resolve, 0));
    }

    const bootState = adapter.getAttribute('data-p2-boot');
    const bootDetail = adapter.getAttribute('data-p2-boot-detail') || '';
    const drawn = Number(adapter.getAttribute('data-p2-drawn') || 0);
    const uiElements = adapter.getAttribute('data-p2-ui-elements');
    const fontFamily = adapter.fontFamily();

    // 二进制读取验证：fetch(...).arrayBuffer() 必须与磁盘字节逐字节一致。
    // （只断言"图片数量 > 0"是不够的——文本近似同样能让计数通过，却会破坏真实贴图。）
    // 包内没有 PNG 时（瘦首包，或整个 pack/ 都从远端取）探针改从**资源包**里挑，
    // 比较基准也换成那一份——验证的是"下载回来的字节与 CDN 上的一致"，而不是自欺欺人地跳过。
    let binaryProbe = 'none';
    const localPng = findPackBinary();
    const probeFromPack = !localPng && remoteMode !== 'off';
    const binaryPath = probeFromPack ? findPackBinary(remoteServeRoot, remoteServeRoot) : localPng;
    const binaryDiskRoot = probeFromPack ? remoteServeRoot : root;
    if (expectPackFailure) {
      // 分包永远挂不上：pack/ 里的二进制在这条用例里按设计就取不到（这正是被测的行为本身）。
      // 这里显式标注而不是静默跳过——否则输出行里 binary=none 会被读成"漏验了"。
      var binaryFailure = null;
      binaryProbe = 'skipped(pack-unreachable)';
    } else if (!binaryPath) {
      var binaryFailure = probeFromPack
        ? '资源包里没有 PNG，无法验证二进制读取'
        : '发布包 pack/ 里没有 PNG，无法验证二进制读取';
    } else {
      try {
        const response = await globalThis.fetch(binaryPath);
        if (!response || !response.ok) {
          var binaryFailure = `二进制资源 fetch 失败: ${binaryPath}`;
        } else {
          const got = Buffer.from(await response.arrayBuffer());
          const onDisk = fs.readFileSync(path.join(binaryDiskRoot, binaryPath));
          if (!got.equals(onDisk)) {
            var binaryFailure = `二进制读取与基准不一致: ${binaryPath}（${got.length} vs ${onDisk.length} 字节）`;
          } else if (probeFromPack && fs.existsSync(path.join(root, binaryPath))) {
            var binaryFailure = `包内不该有 ${binaryPath}（否则这个用例没验证远端路径）`;
          } else {
            binaryProbe = `${binaryPath}(${onDisk.length}B${probeFromPack ? ',remote' : ''})`;
          }
        }
      } catch (error) {
        var binaryFailure = `二进制读取抛错: ${binaryPath} — ${String((error && error.message) || error)}`;
      }
    }

    const failures = [];
    const remoteStats = adapter.remoteStats ? adapter.remoteStats() : null;
    const elapsedMs = Date.now() - remoteStartedAt;

    // --- 远端回落的**反向**用例：期望失败，且必须失败得"快而响"。
    // （--expect-image-band 交给下面的"警示条"用例：那里关心的是"屏幕有没有自证"。）
    if ((remoteMode === 'corrupt' || remoteMode === 'down') && !expectImageBand) {
      const negativeFailures = [];
      if (!bootOutcome.error && (bootState === 'running' || bootState === 'ok')) {
        negativeFailures.push(`期望启动失败，但 boot=${bootState}（启动 Promise 也未 reject）`);
      }
      if (!bootOutcome.error) negativeFailures.push('启动 Promise 没有 reject，无法确认失败被上报');
      if (!remoteStats) {
        negativeFailures.push('适配层没有暴露远端统计');
      } else if (remoteMode === 'corrupt') {
        // 服务端只翻转 1 个字节且长度不变：能被拦下才说明校验和真的生效（而不是靠大小检查）。
        if (remoteStats.checksumMismatch < 1) negativeFailures.push(`期望校验和不符被拦下，实际 checksumMismatch=${remoteStats.checksumMismatch}`);
        // 关键：被拒绝的文件**不能留在缓存里**。早期实现让宿主直接下到缓存路径，
        // 结果"已丢弃"只是口头上的，后续读取照样命中脏文件（实测被这条断言抓到）。
        const poisoned = path.join(userRoot, 'pack', 'project.toml');
        if (fs.existsSync(poisoned)) negativeFailures.push(`被篡改的文件仍留在缓存里: ${poisoned}`);
      } else if (remoteStats.remoteFailed < 1) {
        negativeFailures.push(`期望远端下载失败被记录，实际 remoteFailed=${remoteStats.remoteFailed}`);
      }
      if (stats.remoteAttempts < 1) negativeFailures.push('远端模式下没有发起任何下载请求');
      if (elapsedMs > 20000) negativeFailures.push(`失败得太慢（${elapsedMs}ms），可能缺少超时保护`);
      console.error = originalError;
      restoreRemoteConfig();
      if (negativeFailures.length) {
        fail(`远端回落反向用例未按预期失败（--remote ${remoteMode}）:\n  - ${negativeFailures.join('\n  - ')}`);
      }
      console.log(`wechat_minigame_harness_negative_ok case=${remoteMode === 'corrupt' ? 'checksum-corrupt' : 'remote-unreachable'} boot=${bootState} download_attempts=${stats.remoteAttempts} checksum_mismatch=${remoteStats ? remoteStats.checksumMismatch : 'n/a'} remote_failed=${remoteStats ? remoteStats.remoteFailed : 'n/a'} elapsed_ms=${elapsedMs}`);
      if (remote.server) await remote.server.close();
      return 0;
    }

    // --- 上屏画布来源：`wx.createCanvas()` 只有首次调用返回上屏画布，之后返回离屏画布
    //   （引导代码先调过一次就会这样）。适配层必须挑全局那个上屏画布，否则"流程全绿、
    //   画面全黑"——这是本次真机/开发者工具故障里最可能的一类。这里用 --host-canvas offscreen
    //   让两者**不是同一个对象**，然后断言绘制真的落在上屏画布上、离屏那张一个绘制都没有。
    if (hostCanvasMode === 'offscreen') {
      const canvasFailures = [];
      const info = adapter.canvasInfo ? adapter.canvasInfo() : null;
      if (!info) canvasFailures.push('适配层没有暴露 canvasInfo()');
      else {
        if (info.source !== 'platform-global') canvasFailures.push(`没有选择全局上屏画布（source=${info.source}）`);
        if (!info.isMain) canvasFailures.push('canvas 不是全局上屏画布（isMain=false）');
      }
      if (adapter.canvas !== canvas) canvasFailures.push('适配层的 canvas 不是宿主的上屏画布');
      if (canvas.width !== 1280 || canvas.height !== 720) canvasFailures.push(`上屏画布尺寸未设置（${canvas.width}x${canvas.height}）`);
      const mainOps = canvas.__ctx ? canvas.__ctx.__ops : 0;
      const offscreenOps = offscreenCanvas.__ctx ? offscreenCanvas.__ctx.__ops : 0;
      if (mainOps <= 0) canvasFailures.push('上屏画布上没有任何绘制调用（画到看不见的地方去了）');
      if (offscreenOps > 0) canvasFailures.push(`离屏画布上有 ${offscreenOps} 次绘制（不该画在离屏画布上）`);
      if (stats.frames !== frames) canvasFailures.push(`只执行了 ${stats.frames}/${frames} 帧`);
      restoreRemoteConfig();
      if (canvasFailures.length) {
        fail(`上屏画布选择用例未按预期表现（--host-canvas offscreen）:\n  - ${canvasFailures.join('\n  - ')}`);
      }
      console.log(`wechat_minigame_harness_canvas_ok case=offscreen source=${info ? info.source : 'n/a'} main_ops=${mainOps} offscreen_ops=${offscreenOps} frames=${stats.frames} drawn=${drawn}`);
      if (remote.server) await remote.server.close();
      return 0;
    }

    // --- 屏幕自证覆盖层：小游戏宿主没有 DOM 控制台，"有没有东西真的画到屏幕上"必须能自证。
    //   `--debug-overlay` 打开覆盖层后断言三件事：像素回读命中（画上去的 #ff00ff 读回来了）、
    //   四角测试图案真的落在上屏画布的像素上、诊断快照里的字段是**实测值**而不是占位符。
    if (debugOverlay) {
      const overlayFailures = [];
      const diag = adapter.diag ? adapter.diag() : null;
      if (!diag) overlayFailures.push('适配层没有暴露 diag()');
      else {
        if (!diag.overlay) overlayFailures.push('覆盖层没有被开启（debug-overlay 开关没生效）');
        if (!/命中/.test(String(diag.pixel_probe))) overlayFailures.push(`像素回读没有命中上屏画布: ${diag.pixel_probe}`);
        if (diag.ctx !== '2d-ok' && !noFrames) overlayFailures.push(`ctx 自检异常: ${diag.ctx}`);
        if (!(Number(diag.frames) > 0) && !noFrames) overlayFailures.push(`帧计数为 ${diag.frames}`);
        if (diag.canvas_is_main) overlayFailures.push('本用例不该有全局主画布（前提不成立）');
        if (!String(diag.images).includes('ok=')) overlayFailures.push(`贴图统计缺失: ${diag.images}`);
        // 关键：自证绘制**不能依赖帧循环**。`--no-frames` 下一帧都没跑，四角图案仍须存在，
        // 否则"屏幕全黑"就永远分不清是没画还是画错了地方。
        if (noFrames && !diag.probe_at_start) overlayFailures.push('没有在帧循环之前先画一次覆盖层（无帧时屏幕上会什么都没有）');
        if (noFrames && Number(diag.frames) !== 0) overlayFailures.push(`--no-frames 下不该有帧（frames=${diag.frames}）`);
      }
      // 四角测试图案：覆盖层用的 #ff0055 必须真的留在像素上（证明"画在游戏画面之上"）。
      const corner = painted.some((rect) => rect.color[0] === 255 && rect.color[1] === 0 && rect.color[2] === 85 && rect.x === 0 && rect.y === 0);
      if (!corner) overlayFailures.push('没有在上屏画布上找到覆盖层的四角测试图案（#ff0055 @0,0）');
      const logs = consoleLogs.filter((line) => /渲染自证/.test(String(line)));
      if (!logs.length) overlayFailures.push('没有输出"渲染自证"日志（真机上无法复制现场）');
      // 现场导出通道：开发者工具控制台的"上下文"可能**看不到游戏全局**
      // （实测：`适配层已就绪` 打了、`__P2_DIAG()` 却报 is not defined），所以"挂在 wx 上"
      // 和"写进 wx 存储"这两条不依赖全局作用域的通道必须真的可用。
      if (typeof globalThis.wx.__p2Diag !== 'function') overlayFailures.push('wx.__p2Diag 不可用（控制台看不到游戏全局时的现场通道）');
      let storedDiag = null;
      try { storedDiag = globalThis.wx.getStorageSync('p2_diag'); } catch (e) { storedDiag = null; }
      if (!storedDiag || !/"boot"/.test(String(storedDiag))) overlayFailures.push('wx 存储里没有 p2_diag 快照（存储面板看不到现场）');
      restoreRemoteConfig();
      if (overlayFailures.length) {
        fail(`渲染自证覆盖层用例未按预期表现（--debug-overlay）:\n  - ${overlayFailures.join('\n  - ')}`);
      }
      console.log(`wechat_minigame_harness_overlay_ok frames=${diag.frames} probe=${diag.pixel_probe.replace(/\s+/g, ' ')} ctx=${diag.ctx} corner=yes diag_logs=${logs.length} wx_channel=ok storage=ok images=${String(diag.images).replace(/\s+/g, '_')} boot=${diag.boot}`);
      if (remote.server) await remote.server.close();
      return 0;
    }

    // --- 远端不可达时"不许静默黑屏"：包内骨架读得到、重资源拿不到 → 贴图必然失败。
    //   屏幕必须出现红色警示条，而不是"少了一半素材却没有任何提示"（那就等于黑屏且无解释）。
    if (expectImageBand) {
      const bandFailures = [];
      const diag = adapter.diag ? adapter.diag() : null;
      if (remoteMode !== 'png-fail') bandFailures.push('--expect-image-band 需要配合 --remote png-fail');
      if (diag) {
        const missingMatch = /未供应=(\d+)/.exec(String(diag.images));
        const failMatch = /fail=(\d+)/.exec(String(diag.images));
        const missing = missingMatch ? Number(missingMatch[1]) : 0;
        const failed = failMatch ? Number(failMatch[1]) : 0;
        if (missing + failed < 1) bandFailures.push(`贴图没有被记为失败/未供应: ${diag.images}`);
      } else {
        bandFailures.push('适配层没有暴露 diag()');
      }
      const band = painted.find((rect) => rect.color[0] === 160 && rect.color[1] === 0 && rect.color[2] === 0 && rect.x === 0 && rect.y === 0);
      if (!band) bandFailures.push('屏幕上没有出现红色警示条（贴图静默丢失 = 黑屏且无解释）');
      // 启动失败时不会有游戏帧，警示条由启动失败画面负责画；启动成功则必须跑满帧。
      if (!bootOutcome.error && stats.frames !== frames) bandFailures.push(`只执行了 ${stats.frames}/${frames} 帧`);
      if (elapsedMs > 20000) bandFailures.push(`太慢（${elapsedMs}ms），缺少超时保护`);
      restoreRemoteConfig();
      if (bandFailures.length) {
        fail(`贴图失败警示条用例未按预期表现（--remote png-fail --expect-image-band）:\n  - ${bandFailures.join('\n  - ')}`);
      }
      const samples = (diag.image_samples && diag.image_samples.length)
        ? ' sample=' + String(diag.image_samples[0]).replace(/\s+/g, '_')
        : '';
      console.log(`wechat_minigame_harness_negative_ok case=image-failure-band band=yes boot=${bootState} images=${String(diag.images).replace(/\s+/g, '_')}${samples} frames=${stats.frames} elapsed_ms=${elapsedMs}`);
      if (remote.server) await remote.server.close();
      return 0;
    }

    // --- 宿主连 Image.src 都不让改写（旧基础库/某些真机）：
    //   远端模式必须**响亮失败**（贴图会静默丢失，绝不能当作成功）；
    //   包内完整时（未启用远端）不依赖改写，必须照常启动。
    if (frozenImageSrc) {
      const frozenFailures = [];
      // 需要改写 src 的形态都必须响亮失败：远端回落（重资源不在包内）与**分包形态**
      // （首屏贴图被切到 mainpack/，运行时要的是 pack/ 前缀）——不改写就是整批丢图。
      const expectFailure = remoteMode !== 'off' || (declaredPackSubpackage && fs.existsSync(path.join(root, 'mainpack.json')));
      const patchLog = consoleErrors.find((line) => /不允许改写 Image.src/.test(String(line)));
      if (expectFailure) {
        if (!patchLog) frozenFailures.push('适配层没有报告"不允许改写 Image.src"，问题是静默的');
        if (!bootOutcome.error && (bootState === 'running' || bootState === 'ok')) {
          frozenFailures.push(`期望启动失败，但 boot=${bootState}（远端模式下贴图会静默丢失）`);
        }
        // 响亮失败必须发生在**丢图之前**：如果已经有贴图静默 onerror，说明失败太晚，
        // 画面已经少东西了（这正是"静默降级"最坏的样子）。
        if (stats.imageFailures > 0) frozenFailures.push(`失败得太晚：已有 ${stats.imageFailures} 张贴图静默失败`);
      } else {
        if (bootState !== 'running' && bootState !== 'ok') frozenFailures.push(`包内完整时不该失败，实际 boot=${bootState}`);
        if (bootOutcome.error) frozenFailures.push(`包内完整时启动了却被拒绝: ${String((bootOutcome.error && bootOutcome.error.message) || bootOutcome.error)}`);
        if (stats.imageFailures > 0) frozenFailures.push(`包内完整时不该有贴图失败，实际 ${stats.imageFailures} 张`);
      }
      console.error = originalError;
      restoreRemoteConfig();
      if (frozenFailures.length) {
        fail(`Image.src 冻结用例未按预期表现（--image-src-frozen, --remote ${remoteMode}）:\n  - ${frozenFailures.join('\n  - ')}`);
      }
      console.log(`wechat_minigame_harness_negative_ok case=image-src-frozen remote=${remoteMode} boot=${bootState} loud=${patchLog ? 'yes' : 'no'} image_failures=${stats.imageFailures} elapsed_ms=${elapsedMs}`);
      if (remote.server) await remote.server.close();
      return 0;
    }

    // --- 宿主自带残缺/不可写的全局（微信开发者工具的壳实测如此）：
    //   stub    → 适配层必须**就地补齐**宿主对象并照常启动（绝不能让 document.getElementById
    //             在运行时才变成 "is not a function"）；
    //   locked  → 更狠的一档：全局不可写 + 对象**不可扩展**（只有已存在的自有属性还能重定义）。
    //             适配层绝不能在加载期抛错——那会让 game.js 整体中止、连 __RustLuaWechat 都
    //             不存在，屏幕上只剩黑屏（真机实测踩过）。它必须把能补的补上（getElementById）、
    //             把补不动的**报出来**，然后照常把游戏跑起来。
    //   frozen  → 补不齐必须**响亮**：console.error 给出明确诊断（同时禁止在加载期抛错）。
    if (hostDocument || hostWindowMode) {
      const hostFailures = [];
      const patchLog = consoleWarns.find((line) => /宿主已占用全局/.test(String(line)));
      const expectLoudFailure = hostDocument === 'frozen';
      if (expectLoudFailure) {
        // 冻结的宿主对象补不齐 getElementById → 运行时的 DOM 调用必然 TypeError。
        // 期望：加载期**不抛**（否则黑屏无线索），但 console.error 明确报出"补不齐"，
        // 并且启动以失败收场（失败原因上屏），一帧都不该跑。
        const diagnosable = consoleErrors.find((line) => /被宿主占用且补不齐/.test(String(line)));
        if (!diagnosable) hostFailures.push('适配层没有为"补不齐"给出明确诊断（console.error 里找不到）');
        if (!bootOutcome.error && bootState !== 'failed') {
          hostFailures.push(`期望启动失败（getElementById 补不上），实际 boot=${bootState} 且启动未 reject`);
        }
        if (stats.frames > 0) hostFailures.push(`补不齐时不该继续跑帧（frames=${stats.frames}）`);
      } else {
        if (bootOutcome.error) hostFailures.push(`宿主提供可补齐的全局时不该失败: ${String((bootOutcome.error && bootOutcome.error.message) || bootOutcome.error).slice(0, 200)}`);
        if (bootState !== 'running' && bootState !== 'ok') hostFailures.push(`data-p2-boot=${bootState}（期望 running/ok）`);
        if (!patchLog) hostFailures.push('适配层没有报告"就地补齐"（无法确认走的是补齐而不是替换/丢弃）');
        // 模块必须**加载完整**：开发者工具里 __RustLuaWechat is not defined 就是加载期中止的信号。
        if (!globalThis.__RustLuaWechat || typeof globalThis.__RustLuaWechat !== 'object') {
          hostFailures.push('适配层模块没有加载完（__RustLuaWechat 不存在/不是对象）');
        }
        // 必须是"就地补齐"：宿主对象本身要留在全局上，否则宿主自己的代码会拿到另一个对象。
        if (hostDocument && globalThis.document !== hostStubs.document) {
          hostFailures.push('适配层替换了宿主的 document（期望就地补齐同一个对象）');
        }
        if (typeof globalThis.document.getElementById !== 'function') {
          hostFailures.push('document.getElementById 仍不是函数（这正是开发者工具里报的 TypeError）');
        }
        const canvasFromDocument = globalThis.document.getElementById('p2-canvas');
        if (!canvasFromDocument || typeof canvasFromDocument.getContext !== 'function') {
          hostFailures.push('document.getElementById("p2-canvas") 没有返回可用的 canvas');
        }
        // 运行时的标记落在 `document.body || document.documentElement`：两者**至少一个**能写。
        const markHost = [globalThis.document.body, globalThis.document.documentElement]
          .filter((node) => node && typeof node.setAttribute === 'function');
        if (!markHost.length) {
          hostFailures.push('document.body 与 documentElement 都不能 setAttribute（运行时写启动标记会抛 TypeError）');
        }
        // 诊断不能撒谎：补齐后的标记宿主必须真的接住了运行时的标记（boot/drawn 是画面自证的依据）。
        const hostDiag = adapter.diag ? adapter.diag() : null;
        if (!hostDiag) hostFailures.push('适配层没有暴露 diag()');
        else {
          if (hostDiag.boot !== 'running' && hostDiag.boot !== 'ok') hostFailures.push(`补齐后 data-p2-boot=${hostDiag.boot}（标记没写进 body/documentElement）`);
          if (!(Number(hostDiag.drawn) > 0)) hostFailures.push(`补齐后 data-p2-drawn=${hostDiag.drawn}（画面自证读不到像素数）`);
          if (hostDiag.boot_marks === 'none') hostFailures.push('诊断报告"标记写于 none"（读不到运行时的标记）');
          if (hostDocument === 'locked') {
            // 不可扩展的宿主对象：补不动的成员必须被明确报出来，而不是假装补齐了。
            const blocked = (hostDiag.document_shim && hostDiag.document_shim.blocked) || [];
            if (!blocked.length) hostFailures.push('不可扩展的宿主 document 上没有任何"补不动"的记录（期望有）');
            if (!((hostDiag.document_shim && hostDiag.document_shim.patched) || []).includes('getElementById')) {
              hostFailures.push('getElementById 没有被记入"已就地补齐"');
            }
          }
        }
        if (hostWindowMode === 'separate') {
          if (globalThis.window !== hostStubs.window) hostFailures.push('适配层替换了宿主的 window（期望就地补齐）');
          if (typeof globalThis.window.addEventListener !== 'function') hostFailures.push('宿主 window 上没有事件通道（运行时注册监听会抛错）');
        }
        // 事件必须真的到达运行时：走的是上面已经跑过的触摸/键盘桥接证据。
        if (!drawnEvents.includes('ArrowUp')) hostFailures.push(`宿主全局下的键盘事件未桥接（实际: ${JSON.stringify(drawnEvents)}）`);
        if (!stickKeys.includes('w')) hostFailures.push(`宿主全局下的触摸摇杆未桥接（实际: ${JSON.stringify(stickKeys)}）`);
      }
      restoreRemoteConfig();
      if (hostFailures.length) {
        fail(`宿主全局用例未按预期表现（--host-document ${hostDocument || '-'}, --host-window ${hostWindowMode || '-'}）:\n  - ${hostFailures.join('\n  - ')}`);
      }
      if (expectLoudFailure) {
        const loud = consoleErrors.find((line) => /被宿主占用且补不齐/.test(String(line)));
        console.log(`wechat_minigame_harness_negative_ok case=host-document-frozen boot=${bootState} loud=${loud ? 'yes' : 'no'} frames=${stats.frames} elapsed_ms=${elapsedMs}`);
      } else {
        const hostDiagOk = adapter.diag ? adapter.diag() : {};
        const blocked = ((hostDiagOk.document_shim && hostDiagOk.document_shim.blocked) || []).length;
        console.log(`wechat_minigame_harness_host_globals_ok document=${hostDocument || 'none'} window=${hostWindowMode || 'none'} boot=${bootState} patched=yes getElementById=ok canvas=ok marks=${hostDiagOk.boot_marks || 'n/a'} blocked=${blocked} input=ok frames=${stats.frames} drawn=${drawn}`);
      }
      if (remote.server) await remote.server.close();
      return 0;
    }

    if (bootState !== 'running' && bootState !== 'ok') failures.push(`data-p2-boot=${bootState}（期望 running/ok）`);
    // --- 「点开始新游戏」这条用户真实路径的**世界状态**断言。
    // 只断言"帧循环没停"是不够的：进了图但世界里一个实体都没有、主角没被搬到落点，
    // 这些半死状态以前照样全绿（实测就是"停在标题后又进了 manor，实体 28 个里 NPC
    // 一个都没生成"）。运行时每帧自报 data-p2-world，这里逐项核对。
    // 断言的措辞保持平台通用：不认工程名，只核对宿主 API 的契约。
    // --- 「按住方向键主角会走」：世界起来之后的**运动**断言。
    // 前面所有断言只看"世界存在"（地图对、实体数对、主角在落点）。一个只存在于纸面上的
    // 世界也能全绿：physics.set_position / transform.set 只写日志不落位、或者回写通道的
    // T 行不生效时，画面上的主角钉死在出生点——"能进图但不能玩"就是这种半死状态。
    let moveEvidence = 'not-driven';
    if (driveMove > 0) {
      const readWorld = () => String(adapter.getAttribute('data-p2-world') || '');
      const readPlayer = (text) => {
        const match = text.match(/player=\S+?@(-?\d+),(-?\d+)/);
        return match ? { x: Number(match[1]), y: Number(match[2]) } : null;
      };
      const pump = async () => {
        const callback = rafQueue.shift();
        if (!callback) return false;
        callback(Date.now());
        stats.frames++;
        perFrameDraws.push(stats.drawOps);
        await new Promise((resolve) => setTimeout(resolve, 0));
        return true;
      };
      const before = readPlayer(readWorld());
      // 按哪个键由**运行时自己产出**（触摸摇杆派发出来的键），用例不硬编码游戏按键。
      const keys = stickKeys.length ? stickKeys : ['w'];
      if (!before) {
        failures.push(`--drive-move 前拿不到主角坐标（data-p2-world=${readWorld()}）`);
        moveEvidence = 'no-player';
      } else {
        for (const key of keys) if (inputHandlers.onKeyDown) inputHandlers.onKeyDown({ key, code: key });
        for (let i = 0; i < driveMove; i++) { if (!(await pump())) break; }
        for (const key of keys) if (inputHandlers.onKeyUp) inputHandlers.onKeyUp({ key, code: key });
        // 松键后再泵一帧：自报属性是每帧刷新的，最后一帧的位移要等这一帧才看得见。
        await pump();
        const after = readPlayer(readWorld());
        if (!after) {
          failures.push(`--drive-move 后拿不到主角坐标（data-p2-world=${readWorld()}）`);
          moveEvidence = 'no-player-after';
        } else {
          const dx = after.x - before.x;
          const dy = after.y - before.y;
          moveEvidence = `keys=${keys.join('+')} before=${before.x},${before.y} after=${after.x},${after.y} d=${dx},${dy}`;
          if (dx === 0 && dy === 0) {
            failures.push(`按住 ${keys.join('+')} 共 ${driveMove} 帧后主角一动没动（仍在 ${before.x},${before.y}）` +
              '（输入桥接 → physics.set_position/transform.set → 回写通道 这条链路没生效）');
          }
        }
      }
    }
    const worldSummary = String(adapter.getAttribute('data-p2-world') || '');
    const worldField = (name) => {
      const match = worldSummary.match(new RegExp(name + '=([^\\s]*)'));
      return match ? match[1] : '';
    };
    if (driveNewGame) {
      if (!worldSummary) {
        failures.push('--drive-new-game 后 data-p2-world 是空的（运行时没有自报世界状态）');
      } else {
        const mapName = worldField('map');
        if (!mapName || mapName === 'main') {
          failures.push(`--drive-new-game 后仍在启动场景（map=${mapName || '(空)'}）—— scene.load / map.goto 的排队切换没生效`);
        }
        const sceneCount = Number(worldField('scene') || 0);
        const aliveCount = Number(worldField('alive') || 0);
        if (!(aliveCount > 0)) {
          failures.push(`--drive-new-game 后没有任何存活实体（alive=${aliveCount}）`);
        } else if (aliveCount < sceneCount) {
          failures.push(`--drive-new-game 后存活实体 ${aliveCount} 少于场景声明的 ${sceneCount}（实体表在切换中丢了）`);
        }
        // 「运行期生成实体」这条路径（entity.spawn + sprite.set + animation.attach）：
        // 只有它能把敌人/NPC 这类不在场景里写死的角色变出来。
        if (expectRuntimeSpawns && !(aliveCount > sceneCount)) {
          failures.push(`期望运行期生成实体（alive > scene），实际 alive=${aliveCount} scene=${sceneCount}` +
            '（entity.spawn / find_id / sprite.set / animation.attach 这条链路没生效）');
        }
        // 主角必须落在**地图落点**上：physics.set_position 只打日志不搬人时，
        // 主角会留在场景声明的位置，画面看着"有个人"但玩法全错。
        const spawn = worldField('spawn');
        const player = worldSummary.match(/player=(\S+?)@(-?\d+),(-?\d+)/);
        if (!player) {
          failures.push(`--drive-new-game 后拿不到主角坐标（data-p2-world=${worldSummary}）`);
        } else if (spawn && spawn !== 'none') {
          const [spawnX, spawnY] = spawn.split(',').map(Number);
          if (Math.abs(Number(player[2]) - spawnX) > 2 || Math.abs(Number(player[3]) - spawnY) > 2) {
            failures.push(`主角没落在落点 (${spawnX},${spawnY})，实际 (${player[2]},${player[3]})` +
              '（physics.set_position 没有真的搬动刚体/ECS）');
          }
        }
        // 主角在**渲染器**那一侧的位置必须与 Lua 世界一致。两份数据是分开存的，靠回写
        // 通道（T 行）同步；只对 Lua 侧取样的话，"世界里在走、画面上主角不动"这类断链
        // 完全看不见（正是"能进图但玩不了"的典型症状）。
        const drawn = worldSummary.match(/drawn=(\S+?)@(-?\d+),(-?\d+)/);
        if (!drawn) {
          failures.push(`--drive-new-game 后没有渲染器侧的主角位置自报（data-p2-world=${worldSummary}）`);
        } else if (player && (Math.abs(Number(drawn[2]) - Number(player[2])) > 2 || Math.abs(Number(drawn[3]) - Number(player[3])) > 2)) {
          failures.push(`Lua 世界里的主角在 (${player[2]},${player[3]})，渲染器侧却在 (${drawn[2]},${drawn[3]})` +
            '（回写通道的 T 行没把位置同步给渲染器）');
        }
      }
    }
    // 缺失宿主 API 必须**只有已知可缺的那批**：新缺口一律失败。
    // 平台不实现的宿主 API（这里全是游戏用 type(x)=="function" 探测过、会静默降级的）
    // 记在白名单里；不在这张表里的缺失意味着"某条游戏行为被静默跳过了"。
    const MISSING_HOST_API_ALLOWED = new Set([
      'ui.focus_open', 'ui.focus_select', 'ui.focus_index', 'ui.focus_move',
      'ui.focus_close', 'ui.scroll_close',
      'input.nav_settings', 'input.set_nav_settings', 'input.last_device', 'input.action_repeat',
    ]);
    const reportedMissing = new Set();
    for (const line of consoleLogs.concat(consoleWarns, consoleErrors)) {
      const match = String(line).match(/missing-host-api\] ([A-Za-z_][A-Za-z0-9_.]*)/);
      if (match) reportedMissing.add(match[1]);
    }
    const unexpectedMissing = Array.from(reportedMissing).filter(name => !MISSING_HOST_API_ALLOWED.has(name));
    if (unexpectedMissing.length) {
      failures.push(`出现了未在白名单里的缺失宿主 API: ${unexpectedMissing.join(', ')}` +
        '（新增缺口要显式实现，或把它加进 MISSING_HOST_API_ALLOWED 并说明游戏侧如何降级）');
    }

    if (bootOutcome.error) {
      failures.push(`启动 Promise 被拒绝: ${String((bootOutcome.error && bootOutcome.error.message) || bootOutcome.error).slice(0, 200)}`);
    }
    if (stats.frames < 1) failures.push(`未执行任何帧（raf 队列为空）`);
    if (stats.frames !== frames) failures.push(`只执行了 ${stats.frames}/${frames} 帧（帧循环中途停止，通常意味着 Lua 报错）`);
    if (stats.drawOps <= 0) failures.push('没有任何绘制调用（帧循环未真正绘制）');
    // 每一帧都必须继续产生绘制调用：证明 Lua update 循环在持续推进，而不是只渲染了首帧。
    const drawsEveryFrame = perFrameDraws.length > 0 && perFrameDraws.every((ops, index) => (index === 0 ? ops > 0 : ops > perFrameDraws[index - 1]));
    if (!drawsEveryFrame) failures.push(`并非每帧都在绘制（逐帧绘制调用: ${JSON.stringify(perFrameDraws.slice(0, 6))}）`);
    if (bootDetail && /fail|error|missing/i.test(bootDetail)) failures.push(`data-p2-boot-detail 报告失败: ${bootDetail}`);
    if (!(drawn > 0)) failures.push(`data-p2-drawn=${drawn}（期望 > 0，即画面上确有内容）`);
    // `--expect-pack-failure` 是**故意**让分包永远挂不上的用例：那时 pack/ 一个字节都读不到，
    // "没读 pack/ 就失败" 这条在这里无从满足（它由 `--remote` 系列用例负责）。跳过它的同时，
    // 上面那组"失败必须三重可见 + 首屏照常"的断言全部照跑——不是放行，是换一套更强的要求。
    if (stats.packReads <= 0 && !expectPackFailure) failures.push('没有从 pack/ 读取任何文件');

    // --- 真机代码包模型的收口断言（这一组是"本机全绿、真机启动失败"的直接对症药）
    // ① 产物里不得有"会被上传白名单拒掉、又没被 packOptions.ignore 豁免"的文件。
    //    构建期已经断言过一次；这里再断言一次是因为 harness 面对的是**已存在的产物**
    //    （可能是别的形态/旧产物/手工改过的），而这一条正是真机读不到文件的根因。
    {
      const offenders = withoutUploadModel(() => rules.listUnuploadable(fs, path, root))
        .filter((rel) => !isIgnoredPath(rel));
      if (offenders.length > 0) {
        failures.push(`发布包里有 ${offenders.length} 个文件的后缀不在微信上传白名单内（真机根本读不到它们，` +
          `构建期应当已按 code-package.json 改名成 *.txt）: ${offenders.slice(0, 6).join(' | ')}`);
      }
    }
    // ② 适配层**实际尝试读**的代码包路径，必须全部落在"真机会上传"的集合内。
    //    任何一条落在集合外的读取，在真机上都是 permission denied —— 就是启动失败那一类。
    if (unuploadableHits.size > 0) {
      failures.push(`适配层读取了 ${unuploadableHits.size} 个真机上不存在的代码包路径（后缀不在白名单或应改名未改名）: ` +
        Array.from(unuploadableHits).slice(0, 6).join(' | '));
    }
    // ③ 字体是"宿主自己加载"的接口：必须真的按路径读到了 TTF（否则真机上中文静默变系统字体）。
    if (stats.fontMissing > 0) {
      failures.push(`wx.loadFont 拿到的字体路径读不到（${stats.fontMissing} 次）：真机上只会静默退回系统字体`);
    }
    if (stats.fontBadMagic > 0) {
      failures.push(`字体文件内容不是 TTF（${stats.fontBadMagic} 次）：路径读到了但字节不对`);
    }
    // ④ 适配层探了 harness 没实现的 FSM 方法 → 本机无法判断它是否真能用（可能是静默降级）。
    if (fsmUnknownProbes.size > 0) {
      failures.push(`适配层探测了 harness 未实现的 FileSystemManager 方法: ${Array.from(fsmUnknownProbes).join(', ')}` +
        '（要么在 harness 里实现它，要么明确它不影响行为——不能让它静默降级）');
    }
    if (unhandledRejections.length) {
      failures.push(`出现了未处理的 Promise 拒绝 ${unhandledRejections.length} 条（小游戏里表现为彻底静默）: ` +
        unhandledRejections.slice(0, 3).join(' | '));
    }
    // 适配层必须自己探明"宿主接受哪种代码包路径形式"（实测开发者工具里 pack/project.toml
    // 读不到，而上层只报 HTTP 404，根因是路径形式/挂载差异）。探明失败 = 后续读资源必挂。
    // `missing-pack` 用例是**故意**让宿主拒绝一切 pack/ 路径（模拟整个 pack 都不在包内），
    // 此时本来就探不明——只有远端可走，所以那种用例下不要求探明结果。
    const strategies = adapter.readStrategies ? adapter.readStrategies() : { main: adapter.readStrategy(), pack: adapter.readStrategy() };
    const readStrategy = strategies.pack;
    if (!readStrategy && wxPathStyle !== 'missing-pack' && !expectPackFailure) {
      failures.push(`未能探明可用的代码包路径形式（--wx-path-style ${wxPathStyle}）`);
    }
    // `Image.src` 的接管方式必须自证：基础库把它定义在**原型**上，只看 own 描述符的实现会
    // 静默退化成写 `_src`（基础库只在 setSrc 里发起加载）→ 贴图 Promise 永不 settle →
    // 启动卡死、屏幕全黑、0 error。这里断言"确实接管到了原型上的 setter"，而不是"看起来没报错"。
    if (prototypeImageSrc) {
      const takeover = (adapter.diag ? adapter.diag() : {}).image_src_takeover;
      if (takeover !== 'prototype-accessor') {
        failures.push(`--image-src-prototype 下 Image.src 接管方式=${takeover}（期望 prototype-accessor）`);
      }
      if (stats.imagesLoaded <= 0) failures.push('--image-src-prototype 下贴图一张都没加载（宿主没收到 src）');
    }
    // 回调接管同样必须自证：宿主把回调存在**私有字段**里（基础库 `k$` 包装类）时，
    // 用自己的 accessor 盖住 `onload` 会把宿主的桥拆掉——贴图加载成功也传不回运行时，
    // 症状与"src 没交到宿主"一模一样（卡死 + 0 错误 + 全黑）。断言走的是宿主的 setter。
    if (imageCallbackBridge) {
      const cbDiag = adapter.diag ? adapter.diag() : {};
      if (cbDiag.image_callback_takeover !== 'host-setter') {
        failures.push(`--image-callback-bridge 下回调接管方式=${cbDiag.image_callback_takeover}（期望 host-setter，` +
          '用 own accessor 盖住 onload 会拆掉宿主的回调桥）');
      }
      if (stats.imagesLoaded <= 0) failures.push('--image-callback-bridge 下贴图一张都没加载');
    }
    if (stats.imageFailures > 0) {
      // 把适配层自己关于贴图的诊断带出来：否则只剩"28 张失败"，无法判断是"没下载"还是"下载了但读不到"。
      const imageLogs = consoleErrors.filter((line) => /贴图/.test(String(line))).slice(0, 3);
      failures.push(`有 ${stats.imageFailures} 张贴图读取失败（例: ${stats.imageFailureSamples.join(' | ')}）` +
        (imageLogs.length ? `\n    适配层诊断: ${imageLogs.join(' | ')}` : '（适配层没有贴图相关诊断）'));
    }
    if (binaryFailure) failures.push(binaryFailure);
    // 分包声明与调用次数**按形态**断言——三种形态的期望完全相反，这里是最容易"反正都通过"
    // 的地方，所以两个方向都要显式断言，不能只留一条"没有分包声明就一切正常"。
    const gameJsonPath = path.join(root, 'game.json');
    if (declaredPackSubpackage) {
      // subpackage 形态：必须**有且仅有 1 个**分包、root 规范化后是 pack、name 是 pack、
      // 入口 pack/game.js 存在且真的能被执行（挂载时已 require 过）。
      let gameJson = null;
      try { gameJson = JSON.parse(fs.readFileSync(gameJsonPath, 'utf8')); } catch (error) { gameJson = null; }
      if (!gameJson) {
        failures.push(`game.json 无法解析（分包形态的声明必须可读）: ${gameJsonPath}`);
      } else {
        const list = gameJson.subpackages || gameJson.subPackages || [];
        if (list.length !== 1) failures.push(`分包形态期望**有且仅有 1 个**分包，实际 ${list.length} 个: ${JSON.stringify(list)}`);
        const roots = list.map((item) => String((item && item.root) || '').replace(/\/$/, ''));
        if (roots[0] !== 'pack') failures.push(`分包 root 规范化后=${JSON.stringify(roots[0])}（期望 pack）`);
        const names = list.map((item) => String((item && item.name) || ''));
        if (names[0] !== 'pack') failures.push(`分包 name=${JSON.stringify(names[0])}（期望 pack）`);
      }
      if (!fs.existsSync(path.join(root, 'pack', 'game.js'))) {
        failures.push('分包形态缺少分包入口 pack/game.js（微信要求 subpackages[0].root 目录下有 game.js，缺了开发者工具拒绝打开工程）');
      }
      if (packEntryError) failures.push(`分包入口 pack/game.js 不是可执行的 CommonJS: ${packEntryError}`);
      if (stats.subpackageLoads < 1) failures.push('分包形态没有调用过 wx.loadSubpackage（分包永远不会被挂载）');
      if (subpackageMode !== 'never' && !packMounted) {
        failures.push(`--subpackage ${subpackageMode} 下分包没有挂载成功（attempts=${packMountAttempts}）`);
      }
      // **首屏必须全在主包**：启动期间（标题画面上屏之前）的每一次 pack/ 读取都必须命中主包。
      // 被挂载门挡下的那几次就是"首屏清单漏列"的直接探针——漏了就是标题白屏 + 一片 onerror，
      // 而且这条是**本机唯一**抓得住漏列的地方（真机上表现为"首屏要等下载"）。
      if (packDeniedBeforeBoot > 0) {
        failures.push(`启动期间有 ${packDeniedBeforeBoot} 次 pack/ 读取被未挂载的分包挡下` +
          '（首屏清单 mainpack.json 漏列了首屏需要的资源——它本该被切到主包 mainpack/）');
      }
      // 非首屏资源必须真的触发过挂载（否则"分包"只是个摆设，玩法场景的资源读不到）。
      if (driveNewGame && subpackageMode !== 'never' && stats.subpackageLoads < 1) {
        failures.push('--drive-new-game 进了玩法场景，却没有触发任何分包挂载');
      }
      // mainpack 路由必须真的生效：一次都没走主包 = 清单没读到或路由失效（那时首屏全靠分包）。
      const firstScreenInfo = adapter.firstScreenInfo ? adapter.firstScreenInfo() : null;
      if (!firstScreenInfo || firstScreenInfo.form !== 'mainpack+pack') {
        failures.push(`适配层没有读到首屏清单 mainpack.json（form=${firstScreenInfo && firstScreenInfo.form}）`);
      } else {
        if (!(firstScreenInfo.entries > 0)) failures.push('首屏清单是空的（首屏资源无从留在主包）');
        if (!(firstScreenInfo.mainpack_reads > 0)) {
          failures.push('没有任何一次读取走主包 mainpack/（首屏清单没生效，首屏会退化成等分包下载）');
        }
      }
    } else {
      // full / remote 形态：pack/ 是主包里的普通目录，适配层**不应该**调用 wx.loadSubpackage
      // （调用只会在没有分包声明的包里产生假失败噪音）。game.json 里也不该残留 pack 分包声明
      // ——那会把微信的硬要求（root 下必须有入口 game.js）重新引进来。
      if (stats.subpackageLoads !== 0) failures.push(`pack 未声明为分包，但调用了 wx.loadSubpackage ${stats.subpackageLoads} 次（期望 0）`);
      try {
        const gameJson = JSON.parse(fs.readFileSync(gameJsonPath, 'utf8'));
        const declared = (gameJson.subpackages || gameJson.subPackages || []).map((item) => String(item && item.root || '').replace(/\/$/, ''));
        if (declared.includes('pack')) failures.push(`game.json 声明了 pack 分包但这一形态不该有（会引入分包入口要求）: ${gameJsonPath}`);
      } catch (error) {
        failures.push(`game.json 解析失败: ${String((error && error.message) || error)}`);
      }
    }
    // --- 分包状态机的行为断言（只对声明了分包的包有意义）。
    const packInfo = adapter.packInfo ? adapter.packInfo() : null;
    // 两个方向各自自证，缺一不可：
    //   --deny-code-file X  → 必须真的挡住过（否则用例空转，什么都没验证）；
    //   --expect-no-read X  → 必须**一次都没被读过**（否则说明适配层仍在依赖它，等于没修复）。
    for (const denied of denyCodeFiles) {
      const hits = deniedCodeReads.get(denied) || 0;
      if (expectNoReadFiles.includes(denied)) {
        if (hits > 0) {
          failures.push(`--expect-no-read ${denied}：适配层仍然读了它 ${hits} 次（这个文件在真机上是读不到的，` +
            '依赖它等于把功能绑在一个不可用的输入上）');
        }
      } else if (!hits) {
        failures.push(`--deny-code-file ${denied} 没有挡住任何一次读取（用例没有生效：适配层压根没读它）`);
      }
    }
    for (const name of expectNoReadFiles) {
      if (!denyCodeFiles.includes(name)) failures.push(`--expect-no-read ${name} 没有配套开启拒绝读（用例写错了）`);
    }
    if (declaredPackSubpackage && packInfo) {
      // **不变式**：宿主声明了 pack 分包 ⇒ 适配层必须真的去挂过。
      // declared && !attempted 是最危险的一种失败：判据说该挂、却一次都没挂，
      // 分包永远不会就绪，而屏幕/属性上都不会有提示（失败通道只挂在"挂载失败"上）。
      // 实测踩过：判据本身依赖了一个真机读不到的文件（game.json），于是这里静默为假。
      if (!packInfo.attempted) {
        failures.push('适配层判定声明了分包（declared=true）却一次都没调用 wx.loadSubpackage' +
          `（attempted=false, state=${packInfo.state}, name=${packInfo.name || '(空)'}）——分包永远不会就绪，且没有任何提示`);
      }
      if (packInfo.name !== 'pack') {
        failures.push(`适配层解析出的分包名=${JSON.stringify(packInfo.name)}（期望 pack，来自首屏清单 layout.subpackage_root）`);
      }
      if (subpackageMode === 'delay') {
        // 帧驱动的延迟挂载：挂载必须真的**等到**第 N 帧，而不是提前挂上。
        if (packInfo.mount_frame !== packMountFrame) {
          failures.push(`挂载帧不一致：宿主记 ${packMountFrame}，适配层记 ${packInfo.mount_frame}`);
        }
        if (!(packMountFrame >= subpackageDelayFrames)) {
          failures.push(`--subpackage delay ${subpackageDelayFrames} 期望第 ${subpackageDelayFrames} 帧之后才挂载，实际第 ${packMountFrame} 帧`);
        }
        // "首屏不等分包"的直接证据：启动完成（标题画面已画出来）的那一刻，分包**还没挂上**。
        // 注意这里刻意**不**用"挂载前有 pack/ 读取被门挡下"作为证据：适配层的 fetchShim 与
        // Image.src 都会先 await 分包结算再去读，所以未挂载时根本不会发起读取——没有"被挡"
        // 恰恰说明等待逻辑生效了，反过来断言它反而是错的（第一版就写错了这条）。
        if (packMountedAtBootSettle === null) {
          failures.push('拿不到"启动完成时分包是否已挂载"的记录（启动 Promise 从未 settle）');
        } else if (packMountedAtBootSettle) {
          failures.push(`--subpackage delay ${subpackageDelayFrames} 下分包在启动完成之前就挂上了` +
            '（首屏仍然在等分包，这条用例没有验证到"主包先出画面"）');
        }
        if (packDeniedBeforeBoot > 0) {
          failures.push(`启动期间有 ${packDeniedBeforeBoot} 次 pack/ 读取被未挂载的分包挡下（首屏清单漏列）`);
        }
        // 进度条：真机上这是玩家唯一能看到的等待证据，必须真的消费了进度并画到画布上。
        if (!(packInfo.progress_events > 0)) {
          failures.push('--subpackage delay 下适配层没有消费任何 onProgressUpdate（进度条永远是 0%）');
        }
        const barText = drawnTexts.find((text) => text.includes('正在下载资源'));
        if (!barText) {
          failures.push(`--subpackage delay 下进度条文案没有画到画布上（画布上已有文本: ${drawnTexts.slice(0, 6).join(' | ') || '（无）'}`);
        }
      }
      if (packInfo.state === 'failed' || expectPackFailure) {
        // 失败必须**三重可见**，缺一条就等于"玩家看到黑屏、日志里什么都没有"：
        //   ① 属性（自动化/开发者工具可查）② 控制台 ③ 屏幕（真机上唯一可用的那条）。
        if (packInfo.state !== 'failed') failures.push(`--expect-pack-failure 下适配层分包状态=${packInfo.state}（期望 failed）`);
        if (adapter.getAttribute('data-p2-pack') !== 'failed') {
          failures.push(`分包失败没有落到 data-p2-pack 上（实际 ${adapter.getAttribute('data-p2-pack')}）`);
        }
        const packErrorLog = consoleErrors.find((line) => /分包/.test(String(line)));
        if (!packErrorLog) {
          failures.push(`分包失败没有 console.error（控制台上只有: ${consoleErrors.slice(0, 3).join(' | ') || '（无）'}）`);
        }
        const packBandText = drawnTexts.find((text) => text.includes('分包'));
        if (!packBandText) {
          failures.push(`分包失败没有画到屏幕上（真机上玩家只会看到画面缺素材却毫无提示）；画布上已有文本: ${drawnTexts.slice(0, 6).join(' | ') || '（无）'}`);
        }
        // 失败**不能**掀翻首屏：标题画面已经在主包里画出来了，翻掉就等于把可见的失败变成黑屏。
        if (bootState !== 'running' && bootState !== 'ok') {
          failures.push(`分包失败掀翻了启动（data-p2-boot=${bootState}）：首屏资源全在主包，标题必须照常出来`);
        }
        if (!(drawn > 0)) failures.push(`分包失败下首屏没有画出任何内容（data-p2-drawn=${drawn}）`);
      }
      if (subpackageMode === 'fail-once') {
        // 失败后必须能自愈：冷却（用例里压到 0）过后由下一次 pack/ 读取触发重试。
        if (!(packMountAttempts >= 2)) {
          failures.push(`--subpackage fail-once 期望至少 2 次挂载尝试，实际 ${packMountAttempts} 次（失败变成了永久瘫痪）`);
        }
        if (!packMounted) failures.push('--subpackage fail-once 下重试没有成功挂载');
      }
      if (!expectPackFailure && subpackageMode !== 'never' && packMounted && packInfo.state !== 'ready') {
        failures.push(`分包已挂载但适配层状态=${packInfo.state}（期望 ready）`);
      }
    }
    // 包里每个 .js 都必须能按 **script（CommonJS）** 解析：小游戏运行时不支持 ES Module，
    // 顶层 `export` 会让启动直接失败（实测：WAGame.js:1 SyntaxError Unexpected token 'export'，
    // 来源是 wasm-bindgen --target web 的浏览器胶水）。用 vm.Script 而不是 require/import，
    // 因为它始终按脚本解析——`node --check` 会自动识别 ESM，反而检测不到这个问题。
    const scriptFiles = [];
    (function collect(dir) {
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) collect(full);
        else if (/\.js$/i.test(entry.name)) scriptFiles.push(full);
      }
    })(root);
    const nonCommonJs = [];
    for (const file of scriptFiles) {
      try {
        new vm.Script(fs.readFileSync(file, 'utf8'), { filename: file });
      } catch (error) {
        nonCommonJs.push(`${path.relative(root, file).replace(/\\/g, '/')} (${String((error && error.message) || error).split('\n')[0]})`);
      }
    }
    if (nonCommonJs.length) {
      failures.push(`以下脚本不是 CommonJS，微信运行时会在启动时报 Unexpected token 'export': ${nonCommonJs.join('; ')}`);
    }
    if (!stickKeys.includes('w')) failures.push(`触摸虚拟摇杆未派发 'w'（实际: ${JSON.stringify(stickKeys)}）`);
    if (!drawnEvents.includes('ArrowUp')) failures.push(`微信键盘事件未桥接（实际: ${JSON.stringify(drawnEvents)}）`);
    if (fontFamily !== 'WeChatHarnessFont') failures.push(`字体未注册（fontFamily=${fontFamily}）`);
    const fatalLogs = consoleErrors.filter((line) => /Lua runtime failed|startup failed|missing engine host API|traceback/i.test(String(line)));
    if (fatalLogs.length) failures.push(`运行期报错: ${fatalLogs.slice(0, 2).join(' | ')}`);

    // --- 远端证据：包内读不到（auto）或只留骨架（thin）时，重资源必须真的下载、校验、复用。
    if (remoteMode === 'auto' || remoteMode === 'thin') {
      const thin = remoteMode === 'thin';
      if (!remoteStats) {
        failures.push('适配层没有暴露远端统计');
      } else {
        if (remoteStats.remote < 1) failures.push(`没有从远端取得任何文件（remote=${remoteStats.remote}）`);
        if (remoteStats.remoteBytes < 1024) failures.push(`远端下载字节数异常偏小（remoteBytes=${remoteStats.remoteBytes}）`);
        if (remoteStats.checksumMismatch !== 0) failures.push(`校验和不符 ${remoteStats.checksumMismatch} 次`);
        if (remoteStats.verified < 1) failures.push('没有任何文件通过清单校验（校验没真正跑起来）');
        if (stats.cacheWrites < 1) failures.push('没有写入任何缓存文件');
        if (remoteStats.cache < 1) failures.push(`没有命中过缓存（cache=${remoteStats.cache}，说明每次读都重新下载）`);
        if (stats.remoteRequests !== remoteStats.remote) {
          failures.push(`HTTP 请求数(${stats.remoteRequests}) 与采用数(${remoteStats.remote}) 不一致（重复下载或统计失真）`);
        }
        if (remote.server && remote.server.requests.some((request) => request.status !== 200)) {
          failures.push('本地服务返回过非 200（请求路径不对）');
        }
        if (!thin && remoteStats.package !== 0) {
          failures.push(`包内读不到时不应记到包内来源（package=${remoteStats.package}）`);
        }
        if (thin) {
          // 瘦首包的核心契约：骨架走包内（证明"包内优先"不是口号），重资源才走远端。
          if (remoteStats.package < 1) failures.push(`瘦首包应从包内读到骨架文件，实际 package=${remoteStats.package}`);
          // 骨架在**代码包**里的名字要查改名映射：`.toml` 不在微信上传白名单内，构建期已改成
          // `project.toml.txt`（见 scripts/code-package-rules.js）。这里刻意用磁盘事实去核实
          // "改名确实落地了"，所以查的是产物根目录，而不是经过真机模型的那条读取路径。
          for (const logical of ['project.toml', 'manifest.txt']) {
            const hosted = 'pack/' + hostNameOfLogical(logical);
            if (!fs.existsSync(path.join(root, hosted))) {
              failures.push(`瘦首包缺少骨架文件: ${hosted}（逻辑名 pack/${logical}）`);
            }
          }
          for (const heavy of ['pack/assets', 'pack/scenes', 'pack/scripts']) {
            if (fs.existsSync(path.join(root, heavy))) {
              failures.push(`瘦首包仍含重资源目录: ${heavy}（这个用例的前提不成立，等于没验证远端）`);
            }
          }
        }
      }
    }

    console.error = originalError;
    if (failures.length) {
      stats.consoleErrors = consoleErrors;
      restoreRemoteConfig();
      if (remote.server) await remote.server.close();
      fail(`微信小游戏运行验证失败:\n  - ${failures.join('\n  - ')}\n控制台错误: ${consoleErrors.slice(0, 5).join(' | ') || '（无）'}`);
    }
    restoreRemoteConfig();
    if (remote.server) await remote.server.close();
    const remoteEvidence = (remoteMode === 'auto' || remoteMode === 'thin') && remoteStats
      ? ` remote_downloads=${remoteStats.remote} remote_bytes=${remoteStats.remoteBytes} verified=${remoteStats.verified} cache_hits=${remoteStats.cache} package_reads=${remoteStats.package} http_requests=${stats.remoteRequests}`
      : '';
    // 分包证据行：这三个数（挂载次数 / 挂载帧 / 挂载前被挡下的读取次数）是"主包 + 分包"
    // 这套形态在本机唯一可查的形态证据，缺了它们输出行就只剩一句"跑起来了"。
    const finalPackInfo = adapter.packInfo ? adapter.packInfo() : null;
    const uploadEvidence = ` upload=${uploadedStats().uploaded} upload_denied=${codeReadDenied}` +
      ` font=${stats.fontLoaded ? 'ok' : (stats.fontMissing ? 'missing' : (stats.fontBadMagic ? 'bad-magic' : 'not-requested'))}`;
    const subpackageEvidence = declaredPackSubpackage && finalPackInfo
      ? ` subpackage=pack subpackage_loads=${stats.subpackageLoads} pack_state=${finalPackInfo.state}` +
        ` mount_frame=${packMountFrame} denied_before_mount=${packDeniedBeforeMount} denied_before_boot=${packDeniedBeforeBoot}` +
        ` progress_events=${finalPackInfo.progress_events} mainpack_reads=${(adapter.firstScreenInfo ? adapter.firstScreenInfo().mainpack_reads : 0)}` +
        ` entry=${packEntryError ? 'FAIL' : 'ok'}` +
        ` loading_bar=${drawnTexts.some((text) => text.includes('正在下载资源')) ? 'ok' : 'none'}`
      : ` subpackage=none subpackage_loads=${stats.subpackageLoads}`;
    console.log(`wechat_minigame_harness_ok path_style=${wxPathStyle} read_strategy=${readStrategy} frames=${stats.frames} draw_ops=${stats.drawOps} draws_per_frame=${perFrameDraws.length > 0 ? perFrameDraws[0] : 0} drawn=${drawn} ui_elements=${uiElements} pack_reads=${stats.packReads} images=${stats.imagesLoaded} binary=${binaryProbe}${uploadEvidence}${subpackageEvidence} read_strategies=main:${strategies.main || 'none'}/pack:${strategies.pack || 'none'} font=${fontFamily} boot=${bootState} world=${JSON.stringify(worldSummary.replace(/\s+/g, ' '))} move=${JSON.stringify(moveEvidence)} stick_key=w key_bridge=ArrowUp${remoteEvidence}`);
    return 0;
  }).catch((error) => {
    console.error = originalError;
    restoreRemoteConfig();
    fail(`启动失败: ${(error && error.stack) || error}\n控制台错误: ${consoleErrors.slice(0, 5).join(' | ') || '（无）'}`);
  });
}

function fail(message) {
  process.stderr.write(String(message) + '\n');
  process.exit(1);
}

Promise.resolve(main(process.argv.slice(2))).then((code) => {
  process.exit(typeof code === 'number' ? code : 0);
}).catch((error) => {
  fail(String((error && error.stack) || error));
});
