// 微信小游戏宿主适配层（plugins/wechat-minigame/wechat-adapter.js）
//
// 作用：把 `plugins/webgl/web-runtime.js` 这份浏览器风格的 Lua 运行时所需的宿主 API
// （window / document / fetch / Image / FontFace / requestAnimationFrame /
//   localStorage / 键鼠事件）映射到微信小游戏 API（wx.*），使同一份运行时代码在
// 小游戏环境里无需改动即可运行。
//
// 能力边界（同步维护于 plugins/wechat-minigame/README.md，不得夸大）：
// - 游戏逻辑走 wasm 内的 mlua 执行层（Lua 5.4，vendored）+ `pack/` 资源：适配层启动时
//   实例化 wasm（宿主的 `WXWebAssembly.instantiate` 只有异步、代码包路径这一形态），把 6 个
//   Lua 桥导出挂到 `window.__RUSTLUA_LUA_BRIDGE`（`lua_boot` 包装成 `lua_boot_with_reader`），
//   全部 Lua 交互经桥执行——这条路径在小游戏里**真实可运行**；
// - `pkg/game_web.js` + `pkg/game_web_bg.wasm` 随包分发（胶水是 `--target no-modules` 的
//   纯脚本，由 wasm-bridge.js 包成 CommonJS）。适配层在 boot 之前完成实例化，并把结果
//   （用了哪条通道、哪个代码包路径）写进 `data-rustlua-wasm` / `__RUSTLUA_DIAG().wasm`，
//   详见文末「WASM 说明」。
// - 输入：微信 PC 端键盘（wx.onKeyDown/onKeyUp）直接桥接；触摸映射为指针事件，
//   并提供左侧虚拟摇杆（WASD）与右侧按键（space）以保证手机端可玩。
'use strict';

(function () {
  var g = typeof globalThis !== 'undefined' ? globalThis : this;
  // 版本号会打进日志和 __RUSTLUA_DIAG()：开发者工具会缓存代码包，
  // "有没有报错" 分不清旧包新包，只有版本号能分清（实测踩过）。
  var ADAPTER_VERSION = '0.7.12+pixel-sample-switch';
  var wx = g.wx;
  if (!wx) {
    throw new Error('wechat-adapter: 未找到全局 wx —— 本文件只能在微信小游戏运行时中加载');
  }

  // 逻辑分辨率 = 游戏的**横版设计分辨率**（1280×720），drawScene/HUD 依赖 canvas.width/height。
  //
  // 为什么必须是横版：`game_jhyt` 在 M32 一度改成竖版，随后按「横屏方案A」转回横版——Lua 面板
  // 坐标按 `scripts/ui/layout.lua` 的 DESIGN_W/DESIGN_H（1280×720）排，而 web 运行时的绘制是
  // **屏幕像素空间**、不做缩放，画布尺寸不对就是面板溢出画布（超出部分直接看不见）。
  //
  // **改这个尺寸要三处一起改**，缺一处就是"画布与版面不一致"，而那种错位在无头验证里看不出来
  // （假 ctx 不裁剪、坐标超界也照样记数）：
  //   ① 这里（小游戏画布）；② `game.json` 的 `deviceOrientation`（否则宿主按相反方向给窗口）；
  //   ③ webgl 侧 `engine/crates/game_web/src/lib.rs` 的 canvas 尺寸。三者必须等于设计分辨率。
  var VIEW_WIDTH = 1280;
  var VIEW_HEIGHT = 720;
  // `game.json` 里声明的方向（**编译期常量，不读文件**）：`game.json` 上传后被编译，
  // 真机上 `readFileSync` 一律 permission denied，而适配层有一条硬约束"一次都不许读它"
  // （回归守卫 `--expect-no-read game.json`）。所以方向只作为**自述**出现在诊断里，
  // 不与 `game.json` 做运行期比对——那属于构建期校验（见 scripts/package.ps1）。
  var GAME_ORIENTATION = 'landscape';
  // pack/ 是资源根：分包形态（-AssetPack subpackage）下它是 game.json 里声明的分包；
  // 其余形态下它只是主包里的普通目录。是不是分包一律以 game.json 的声明为准（见 hasPackSubpackage）。
  var PACK_PREFIX = 'pack/';
  // 兜底常量：真正的分包名以首屏清单的 layout.subpackage_root 为准（见 packSubpackageName），
  // 这里只用于"清单没给出名字"时的诊断文案与默认值。
  var PACK_SUBPACKAGE = 'pack';
  // 分包形态的**主包首屏目录**：与 pack/ 同级的兄弟目录，构建期按 mainpack.json 把首屏资源搬进去。
  // 为什么不干脆留在 pack/ 里：微信规则规定 subpackages[i].root 目录下的文件**全部**进分包，
  // 留在 pack/ 里的首屏资源在真机上依然要等分包下载 —— "主包留一部分"这个直觉做法根本不成立。
  var MAINPACK_DIR = 'mainpack/';
  var MAINPACK_MANIFEST = 'mainpack.json';
// wasm 核心在代码包里的逻辑路径。**位置随发布形态变**，判据是首屏清单声明没声明分包：
//   · 分包形态 → `pack/game_web_bg.wasm`（在分包里）。微信主包只有 4 MB，而内核 2.5 MB：
//     实测 2026-10-06 主包 4,547 KB（内核 + Lua 与首屏内容），超 451 KB —— 内容侧已经压不动，
//     唯一剩下的杠杆是让内核去分包（逐目录归属见 docs/52「主包构成」）。
//     **代价是启动次序变了**：实例化必须排在 `wx.loadSubpackage` 之后，标题画面因此也排在
//     分包下载之后，于是等待画面必须由 JS 自己画（见 startPreBootUi）。
//   · full / remote 形态 → `pkg/game_web_bg.wasm`（主包）。full 下 pack/ 本身也在主包（搬了
//     等于没搬），remote 下 pack/ 会被构建期裁成骨架（内核搬过去就没了）。
// 胶水 `pkg/game_web.js` 三种形态都留在主包：它由 `require('./pkg/game_web.js')` **同步**加载，
// 进分包要换成"挂载之后再 require"，而那条语义本机证明不了（真机才认），所以不动它。
// `.wasm` 在微信代码包后缀白名单内（scripts/code-package-rules.js），因此不需要改名。
var WASM_MODULE_PATH_MAIN = 'pkg/game_web_bg.wasm';
var WASM_MODULE_PATH_PACK = PACK_PREFIX + 'game_web_bg.wasm';
// EH 真机探针模块（53 字节，零导入 + 本地 tag 段 + legacy try/catch）：由发布链路写进 pkg/，
// wasm-bridge 的 probeEHAcceptance 走同一条代码包通道 compile 它，读"设备引擎认不认 wasm EH"。
var EH_PROBE_PATH = 'pkg/eh-probe.wasm';
// WP10 微信侧接线：HostApi 构造器的发布名（与 webgl 发布名一致，app.name() 的返回值）。
var WECHAT_PUBLISH_NAME = 'jianghu_youtian';
  // 代码包**后缀白名单**的改名映射（构建期生成，产物根；三种形态都有）。
  // 微信代码包只上传后缀在白名单内的文件，`.lua` / `.toml` / `.md` 传不上去——真机上读它们是
  // `permission denied`，而本游戏全部 Lua 逻辑都靠逐文件读取，实测真机启动直接失败。
  // 构建期把这些文件的文件名追加 `.txt`，映射表放在这里；运行时**只查表、不重算规则**
  // （规则在 scripts/code-package-rules.js，构建期与无头验证共用，两处重算必然漂移）。
  var CODE_PACKAGE_MAP = 'code-package.json';
  var userRoot = (wx.env && wx.env.USER_DATA_PATH) || '';
  var sysInfo = safeCall(wx, 'getSystemInfoSync') || {};
  var screenWidth = sysInfo.windowWidth || 375;
  var screenHeight = sysInfo.windowHeight || 667;
  var pixelRatio = sysInfo.pixelRatio || 1;

  function safeCall(target, name, arg) {
    if (!target || typeof target[name] !== 'function') return null;
    try { return target[name](arg); } catch (e) { return null; }
  }

  function log() {
    if (typeof console !== 'undefined' && console.log) console.log.apply(console, arguments);
  }
  function warn() {
    if (typeof console !== 'undefined' && console.warn) console.warn.apply(console, arguments);
  }
  // 响亮失败必须走 console.error 这一档：harness 的负向用例只收 console.error（收 console.warn 的
  // 那份只做统计、不打到输出里），真机的日志面板同理。用 warn 会被"降级为告警"的语义吞掉，
  // 用裸 throw 则会落进调用方的 Promise 里被吞掉（实测：`new Image()` 在 Promise 执行器里，
  // 抛出的异常只让那个 Promise 变 rejected，屏幕上少一批贴图、日志里一条都没有）。
  // **名字不能叫 `error`**：调用点多半在 `catch (error)` 里，那个形参会把同名助手遮蔽掉，
  // 于是 `error(msg)` 变成"调用一个错误对象"，抛 TypeError、什么都打不出来（第 54 轮踩过，
  // 表现是"加了日志但行为一点没变"）。
  function logError() {
    if (typeof console !== 'undefined' && console.error) console.error.apply(console, arguments);
  }

  // ---------------------------------------------------------------- 文件系统

  // 代码包（主包/分包）的读取路径在不同宿主上并不一致：开发者工具、真机、不同基础库版本对
  // "根相对路径 / 前导斜杠 / ./ 前缀 / 分包根相对路径" 的接受程度不同，而错误又被
  // FileSystemManager 吞成异常。所以这里按候选策略逐个尝试、记住成功的那种，
  // 全部失败时把**宿主原始错误**打印出来——否则只能看到上层 runtime 的 "HTTP 404"，
  // 无法判断是路径形式不对、文件没进包，还是（声明了分包时）分包还没挂载。
  // 记住可用形式时按**包**分开记：主包文件与 pack/ 目录的路径语义可能不同
  // （主包读 remote-assets.json 成功，不代表 pack/ 下也能用同一种形式）。
  var preferredStrategy = { main: null, pack: null };
  var failureLogged = {};
  var strategyLogged = { main: false, pack: false };

  var readStrategies = [
    // 用户目录缓存：路径必须与**写入**走同一个函数（writeCache → cachePathFor）。
    // 这里原本自己拼 `userRoot + '/' + p`，于是"写在新路径、读在旧路径"——带发布标识的新
    // 缓存永远读不到，而**不带标识的旧缓存永远被读到**，正是"换了版本真机还是老样子"的
    // 最后一段链路（cachePathFor 加标识只改了一半，另一半在这里）。
    { name: 'user-data', apply: function (p) { return cachePathFor(p); } },
    { name: 'root-relative', apply: function (p) { return p; } },
    { name: 'leading-slash', apply: function (p) { return '/' + p; } },
    { name: 'dot-prefix', apply: function (p) { return './' + p; } },
    // 分包根相对（去掉 pack/ 前缀）：个别宿主把已加载的分包当作独立根。
    // 注意副作用：这条会读成 `x`，若宿主实际按主包根解析，就可能命中同名主包文件
    // （如 pack/game.js → 主包入口）。所以它排在最后，且只在其它形式都失败后才生效。
    { name: 'subpackage-relative', apply: function (p) { return p.indexOf(PACK_PREFIX) === 0 ? p.slice(PACK_PREFIX.length) : null; } }
  ];

  // 首屏清单（分包形态）：构建期由 scripts/first-screen-pack.js 从 pack/ 的实际内容推导，
  // 构建脚本、本适配层与无头 harness 共用同一份。它是"某个逻辑路径该从主包读还是从分包读"的
  // **唯一**依据——运行时只查表、不探测：探测会在分包未挂载时把首屏文件误判成"包内没有"，
  // 进而去等一个本不该等的下载（首屏白屏），正是分包形态要消除的东西。
  var firstScreen = null;
  var firstScreenLoaded = false;
  // 清单**正在加载**的窗口。加载期间不得走 `user-data`（远端缓存）候选：缓存路径要先算
  // **发布标识**，发布标识要读 `pack/manifest.txt`，而 manifest.txt 该读主包还是分包
  // **取决于正在加载的这份清单** —— 一个闭环。闭环里的内层调用看到的是
  // "firstScreenLoaded=true 而 firstScreen=null"，于是原样去打 `pack/manifest.txt`；
  // 分包形态下这个文件已经被切到 mainpack/，读取在分包挂载前被挡。
  // 2026-10-09 实测（无头 harness，分包形态）：启动期 6 次被挡全部来自这条链
  // （栈：readText → loadManifestChecksums → releaseKey）。
  var firstScreenLoading = false;
  var mainpackReads = 0;

  function loadFirstScreenManifest() {
    if (firstScreenLoaded) return firstScreen;
    firstScreenLoaded = true;
    firstScreenLoading = true;
    var text = null;
    try { text = readText(MAINPACK_MANIFEST); } catch (error) { text = null; }
    firstScreenLoading = false;
    if (!text) return null;
    var parsed = null;
    try { parsed = JSON.parse(text); } catch (error) {
      warn('wechat-adapter: ' + MAINPACK_MANIFEST + ' JSON 解析失败 -> ' + hostError(error));
      return null;
    }
    var list = parsed && parsed.entries;
    if (!list || typeof list.length !== 'number') {
      warn('wechat-adapter: ' + MAINPACK_MANIFEST + ' 缺少 entries 列表，按"无首屏清单"处理（不启用主包路由）');
      return null;
    }
    var set = Object.create(null);
    for (var i = 0; i < list.length; i++) set[String(list[i])] = true;
    firstScreen = { entries: set, count: list.length, layout: (parsed && parsed.layout) || {} };
    // 这一行同时自报"要不要挂分包"（判据就是清单里的 layout，见 packSubpackageName）：
    // 出故障时第一眼要能看出来"究竟是没声明分包，还是声明了却没挂上"。
    var subpackage = packSubpackageName();
    log('wechat-adapter: 首屏清单 ' + list.length + ' 条 → 主包 ' + MAINPACK_DIR + ' 直接可读，其余在 ' +
      (subpackage ? subpackage + '/ 分包里按需挂载' : '分包里（但清单未声明 subpackage_root，不挂载）'));
    return firstScreen;
  }

  function firstScreenKeys() {
    return loadFirstScreenManifest();
  }

  // 代码包改名映射：逻辑路径（pack 相对，`/` 分隔）→ 会随代码包上传的宿主文件名。
  // 惰性读一次；读不到就是空表（未改名/旧产物 → 宿主名 == 逻辑名，行为与从前一致）。
  var renameMap = null;
  var renameMapLoaded = false;
  var renameHit = 0;

  function loadRenameMap() {
    if (renameMapLoaded) return renameMap;
    renameMapLoaded = true;
    var text = null;
    try { text = readText(CODE_PACKAGE_MAP); } catch (error) { text = null; }
    if (!text) return null;
    var parsed = null;
    try { parsed = JSON.parse(text); } catch (error) {
      // 解析失败要**响亮**：改名映射读不出来时，91 个 `.lua` 会按原名去读，症状是"分包挂载了却
      // 一个脚本都读不到"，与"文件没上传"长得一样，很难往回推到这里。
      warn('wechat-adapter: ' + CODE_PACKAGE_MAP + ' JSON 解析失败 -> ' + hostError(error));
      return null;
    }
    var table = Object.create(null);
    var count = 0;
    var renamed = parsed && parsed.renamed;
    if (renamed) {
      for (var key in renamed) {
        if (!Object.prototype.hasOwnProperty.call(renamed, key)) continue;
        table[key] = String(renamed[key]);
        count += 1;
      }
    }
    renameMap = { table: table, count: count };
    log('wechat-adapter: 代码包改名映射 ' + count + ' 条（后缀不在微信上传白名单内的文件；运行时按此表查宿主文件名）');
    return renameMap;
  }

  // 逻辑路径 → **会随代码包上传的宿主路径**。只对代码包侧的候选使用；
  // 用户目录缓存路径与远端 CDN URL 一律仍用逻辑路径（CDN 不受白名单约束）。
  function hostFileFor(logicalPath) {
    var map = loadRenameMap();
    if (!map) return logicalPath;
    var host = map.table[logicalPath];
    if (host === undefined) return logicalPath;
    renameHit += 1;
    return host;
  }

  function renamingInfo() {
    var map = loadRenameMap();
    return { map_entries: map ? map.count : 0, lookups_hit: renameHit };
  }

  // 逻辑路径（`pack/<相对路径>`）是否属于首屏（主包侧）。
  function isFirstScreenPath(path) {
    if (path.indexOf(PACK_PREFIX) !== 0) return false;
    var manifest = firstScreenKeys();
    if (!manifest) return false;
    return !!manifest.entries[path.slice(PACK_PREFIX.length)];
  }

  // 这个逻辑路径是否"必须等分包挂载"：pack/ 前缀 + 清单声明了分包（见 packSubpackageName）+ 不在首屏清单里。
  // 用于 Image.src 那条**同步**通道：分包还没挂载就交给宿主，宿主会立刻 onerror，
  // 而运行时的 loadImage 以第一次 onerror 为准——那时再挂载也救不回这张贴图。
  function needsPackMount(path) {
    var normalized = normalizePath(path);
    if (normalized.indexOf(PACK_PREFIX) !== 0) return false;
    if (isFirstScreenPath(normalized)) return false;
    if (packState === 'ready' || packState === 'absent' || packState === 'failed') return false;
    return hasPackSubpackage();
  }

  // 逻辑路径 → **读取主体**。分包形态下 `pack/<x>` 若在首屏清单里，真正该读的是
  // `mainpack/<x>`（构建期搬过去的），否则才是 `pack/<x>`（分包挂载后才可读）。
  //
  // 为什么必须与"方言"分成两层、而不能把 mainpack/ 当成策略表里的一条别名：
  // 方言是**宿主属性**（同一份代码在不同基础库上接受的路径写法不同），主体是**包内布局属性**
  // （文件到底在哪）。别名塞进策略表，在 slash / sub-root 这类宿主上会被拼成 `/mainpack/...`
  // 而读不到 —— 两个正交的维度必须正交地组合。
  //
  // 只返回一个主体，**不回落**到 pack/：清单里的文件构建期已被搬走（package.ps1 有字节守恒
  // 断言兜底），在 pack/ 再找一次只会把"构建没搬干净"这种硬故障掩盖成"分包还没挂载"。
  //
  // 主体里的**文件名**再套一次改名映射（`hostFileFor`）：微信代码包只上传后缀在白名单内的文件，
  // `scripts/main.lua` 这类名字在真机上根本不存在，实际文件名是 `main.lua.txt`。
  // 映射对两个主体都生效（主包侧 mainpack/ 与分包侧 pack/ 用的是同一张表、同一个键域）。
  function readSubjects(path) {
    if (path.indexOf(PACK_PREFIX) !== 0) {
      // 根目录文件（game.json / remote-assets.json / mainpack.json…）不走 pack/ 键域，原样返回。
      return [path];
    }
    var logical = path.slice(PACK_PREFIX.length);
    var host = hostFileFor(logical);
    if (!isFirstScreenPath(path)) return [PACK_PREFIX + host];
    return [MAINPACK_DIR + host];
  }

  // 把已记住的方言提到最前（其余顺序不变）。抽出来是因为 readWithStrategies 与 hostSrcFor
  // 都要用同一套排序，两处各写一遍迟早会漂移。
  function orderStrategies(remembered) {
    var ordered = readStrategies.slice();
    if (!remembered) return ordered;
    for (var i = 0; i < ordered.length; i++) {
      if (ordered[i].name === remembered && i > 0) {
        ordered.unshift(ordered.splice(i, 1)[0]);
        break;
      }
    }
    return ordered;
  }

  // 资源来源统计用的作用域：按**逻辑路径**分。分包形态下首屏文件也算 pack/ 这一套资源
  // （它们是同一批素材，只是被放到主包侧），否则 assetStats.package 会把首屏整批漏计。
  function scopeOf(path) {
    return path.indexOf(PACK_PREFIX) === 0 ? 'pack' : 'main';
  }

  // 方言记忆用的键：按**读取主体**分。理由有两条，都来自真实宿主形态：
  //   ① mainpack/ 下的文件物理上就在 launcher 根下，与 game.json / remote-assets.json 同级，
  //      必须共用同一套路径语义（--wx-path-style root/slash 就是这种"根相对"宿主）；
  //   ② pack/ 是**分包根**，宿主可能给它完全另一套语义（--wx-path-style sub-root 就是这样：
  //      主包里根本没有 pack/ 目录，只有去掉前缀的分包根相对路径能读到）。
  // 两套语义同时存在时，用同一个键会互相覆盖——功能上仍能读通（策略表会逐个回退），
  // 但"已探明的形式"这个诊断就永远是错的，而它正是排查"文本读得到图片 404"的唯一线索。
  function strategyKeyOf(subject) {
    return subject.indexOf(PACK_PREFIX) === 0 ? 'pack' : 'main';
  }

  // ---- 宿主 FS 的**路由包装**：让所有读者共用同一张首屏清单 ----------------------
  //
  // 分包形态把首屏文件切到 mainpack/，判断"该读主包还是分包"的依据只有一条
  // （isFirstScreenPath）。过去它只在「适配层自己的读取通道」（readWithStrategies）里生效，
  // 而 `plugins/webgl/web-runtime.js` 在微信侧是直接调
  // `wx.getFileSystemManager().readFileSync('pack/…')` 的——**整条路由被绕过**。
  // 2026-10-09 实测（分包形态、无头 harness）：启动期它问 `pack/manifest.txt` 六次，全部在
  // 分包挂载前被宿主的挂载门挡下；同一时刻适配层自检读的是 `mainpack/manifest.txt`、报
  // `ok@mainpack`。两条读者、两套语义，正是"同一份 runtime 跑两平台"要消除的裂缝。
  //
  // 约定（每条都是为了"宁可原样失败，也不改变语义"）：
  //   · 只处理 `pack/` 开头的路径，其余一律原样转给宿主；
  //   · 只接管**同步**通道（readFileSync / accessSync）：异步 readFile 的失败走回调，
  //     替它决定"要不要重试"会把宿主的错误文案换掉，得不偿失；
  //   · 路由候选**全失败就回落原调用**，让宿主自己报错（无清单 / full / remote 形态纯直通）；
  //   · 只包一次（`__rustluaRouted`），且内部一律调用**未包装**的原函数 ⇒ 不会递归。
  var fsRouteInstalled = false;
  var fsRouteWrapped = false;
  var fsRouteHits = 0;
  var fsRouteMisses = 0;
  // 已包装过的宿主 FS 对象。**标记不写在宿主对象上**：无头 harness 会枚举 FileSystemManager
  // 的成员，遇到未实现的名字直接判失败（"不能让它静默降级"）——这条守卫是对的，所以包装状态
  // 只能记在这边（见 scripts/wechat-minigame-harness.js 的宿主方法枚举）。
  var fsRouteObjects = [];
  // 诊断用：`fsRouteSkipped` = 有人读了 `pack/…` 却**没有路由可言**（清单没读到）；`fsRouteTried` =
  // 真的按候选表找过。两者必须分开——只报命中数时，"清单没加载"和"候选全失败"长得一模一样。
  var fsRouteSkipped = 0;
  var fsRouteTried = 0;

  // 逻辑路径（`pack/<x>`）的路由候选：与 readWithStrategies 同一张策略表、同一个 `user-data`
  // 特例（缓存键用**逻辑路径**，理由见那里的注释），顺序也一致。
  function routedCandidates(path) {
    var subjects = readSubjects(path);
    var list = [];
    for (var s = 0; s < subjects.length; s++) {
      var ordered = orderStrategies(preferredStrategy[strategyKeyOf(subjects[s])]);
      for (var j = 0; j < ordered.length; j++) {
        // 见 firstScreenLoading 的注释：清单加载期间算不出缓存路径（要发布标识、要 manifest.txt），
        // 强行算就是那个闭环，所以这一窗口里跳过 user-data。
        if (ordered[j].name === 'user-data' && firstScreenLoading) continue;
        var candidate = ordered[j].apply(ordered[j].name === 'user-data' ? path : subjects[s]);
        if (candidate && list.indexOf(candidate) < 0) list.push(candidate);
      }
    }
    return list;
  }

  function shouldRouteFsPath(path) {
    if (typeof path !== 'string' || path.indexOf(PACK_PREFIX) !== 0) return false;
    // 清单没读到 ⇒ 没有路由可言（full / remote 形态正是这条路），保持宿主语义。
    if (!loadFirstScreenManifest()) { fsRouteSkipped += 1; return false; }
    fsRouteTried += 1;
    return true;
  }

  function wrapFsRouting(fs) {
    if (!fs || fsRouteObjects.indexOf(fs) >= 0) return false;
    var origRead = typeof fs.readFileSync === 'function' ? fs.readFileSync : null;
    var origAccess = typeof fs.accessSync === 'function' ? fs.accessSync : null;
    if (!origRead && !origAccess) return false;
    if (origRead) {
      fs.readFileSync = function (path, encoding) {
        var direct = function (target) {
          return encoding === undefined ? origRead.call(fs, target) : origRead.call(fs, target, encoding);
        };
        if (!shouldRouteFsPath(path)) return direct(path);
        var list = routedCandidates(path);
        for (var i = 0; i < list.length; i++) {
          try {
            var value = direct(list[i]);
            if (value !== null && value !== undefined) { fsRouteHits += 1; return value; }
          } catch (error) { /* 试下一条候选 */ }
        }
        fsRouteMisses += 1;
        // 全部失败：原样再问一次宿主，让错误文案与未包装时**完全一致**。
        return direct(path);
      };
    }
    if (origAccess) {
      fs.accessSync = function (path) {
        if (!shouldRouteFsPath(path)) return origAccess.call(fs, path);
        var list = routedCandidates(path);
        for (var i = 0; i < list.length; i++) {
          try { origAccess.call(fs, list[i]); fsRouteHits += 1; return undefined; } catch (error) { /* 下一条 */ }
        }
        fsRouteMisses += 1;
        return origAccess.call(fs, path);
      };
    }
    fsRouteObjects.push(fs);
    return true;
  }

  // 包在 `wx.getFileSystemManager` 这一层（而不是某一次返回的对象上）：宿主每次返回新对象时
  // 包装依然生效。返回对象做缓存，保证同一宿主对象对应同一个包装（身份稳定，便于现场排查）。
  function installFsRouting() {
    if (fsRouteInstalled) return fsRouteWrapped;
    fsRouteInstalled = true;
    var original = null;
    try { if (typeof wx.getFileSystemManager === 'function') original = wx.getFileSystemManager; } catch (error) { original = null; }
    if (!original) return false;
    var seen = [];
    function routedManager() {
      var fs = original.call(wx);
      if (!fs) return fs;
      for (var i = 0; i < seen.length; i++) if (seen[i].raw === fs) return seen[i].routed;
      try { wrapFsRouting(fs); } catch (error) { /* 宿主对象不可写：原样交回 */ }
      seen.push({ raw: fs, routed: fs });
      return fs;
    }
    try {
      wx.getFileSystemManager = routedManager;
      fsRouteWrapped = true;
    } catch (error) {
      // 宿主不许改写：退回"包住当前这一个实例"。功能等价，只是宿主若每次返回新对象就漏掉。
      try { fsRouteWrapped = wrapFsRouting(original.call(wx)); } catch (error2) { fsRouteWrapped = false; }
    }
    // 清单**在这里就加载**（原来只在第一次 pack/ 读取时惰性加载）：启动期最早的几次
    // `pack/…` 读取（web-runtime.js 一进来就问 manifest.txt）会赶在清单加载之前发生，
    // 那些读取只能按原样打到宿主上、在分包挂载前被挡。实测（2026-10-09，分包形态无头 harness）：
    // 6 次被挡里有 2 次属于这一类，`fs_route.skipped_no_manifest` 就是它的计数。
    // 读的是主包里的 mainpack.json（不带 pack/ 前缀、不经过路由），所以不会递归。
    try { loadFirstScreenManifest(); } catch (error) { /* 读不到就维持"无清单"语义 */ }
    return fsRouteWrapped;
  }

  function fsRouteInfo() {
    return {
      installed: fsRouteInstalled,
      wrapped: fsRouteWrapped,
      tried: fsRouteTried,
      skipped_no_manifest: fsRouteSkipped,
      hits: fsRouteHits,
      misses: fsRouteMisses
    };
  }

  function fsManager() {
    return safeCall(wx, 'getFileSystemManager');
  }

  function hostError(error) {
    if (!error) return 'unknown';
    return String(error.errMsg || error.message || error);
  }

  // 逐个候选策略读取；`readOne(fs, candidate)` 返回 null/undefined 表示"这条不通"。
  // 返回值：{ value, source, candidate, subject } 或 null。source ∈ 'cache'(用户数据目录) | 'package'。
  //   candidate = 真正读成功的那个**宿主路径**：wx.loadFont / Image.src 这类由宿主自己加载的接口
  //               必须拿到它，自己按方言重算一遍会与这里探明的结果漂移；
  //   subject   = 命中的读取主体（见 readSubjects），用来区分"读的是主包那份还是分包那份"。
  // `quiet`：重试期间不打印失败详情，只在最后一次尝试时输出（否则每个文件会刷 N 行同样的日志）。
  function readWithStrategies(path, readOne, quiet) {
    var fs = fsManager();
    if (!fs) return null;
    // pack/ 的路由必须早于**任何** pack/ 读取：否则首屏文件会被当成"包内没有"去等分包。
    // 惰性加载一次，之后是纯内存查表（loadFirstScreenManifest 读的是主包文件，不会递归进来）。
    if (path.indexOf(PACK_PREFIX) === 0) loadFirstScreenManifest();
    // 作用域按**逻辑路径**分（不按主体）：分包形态下 mainpack/ 与 pack/ 都是"代码包相对路径"
    // 这一套语义（同一份代码、同一个宿主），资源来源统计跟着逻辑路径走才连续。方言记忆则按
    // **主体**分（strategyKeyOf），理由见那里的注释。
    var subjects = readSubjects(path);
    var failures = [];
    for (var s = 0; s < subjects.length; s++) {
      var scope = strategyKeyOf(subjects[s]);
      var ordered = orderStrategies(preferredStrategy[scope]);
      for (var j = 0; j < ordered.length; j++) {
        // **user-data 这条方言必须作用在逻辑路径上**，其余方言作用在宿主路径（已改名）上。
        // 理由：改名只存在于**代码包**里（`pack/scripts/x.lua` 在真机上叫 `x.lua.txt`），
        // 而用户目录缓存的键是**逻辑路径**（`cachePathFor(logical)` 与远端 URL 都按逻辑路径算）。
        // 早先把两者混为一谈 → 远端下载写 `http://usr/pack/scripts/x.lua`，缓存候选却去找
        // `http://usr/pack/scripts/x.lua.txt`，于是"下载成功了却永远读不到"（瘦首包形态实测踩到）。
        //
        // 清单加载期间跳过它：`cachePathFor` 要算发布标识、发布标识要读 `pack/manifest.txt`，
        // 而 manifest.txt 读哪一侧取决于正在加载的清单（见 firstScreenLoading 的注释）。
        if (ordered[j].name === 'user-data' && firstScreenLoading) continue;
        var candidate = ordered[j].apply(ordered[j].name === 'user-data' ? path : subjects[s]);
        if (!candidate) continue;
        try {
          var value = readOne(fs, candidate);
          if (value !== null && value !== undefined) {
            // user-data 是**缓存来源**，不是"宿主接受的代码包路径形式"：命中它只能说明
            // "这个文件已经下载过"，学成 preferredStrategy 会把方言探明结果覆盖掉
            // （实测：一次远端下载之后 pack 作用域的形式就变成 user-data，再也探不明真方言）。
            if (ordered[j].name !== 'user-data' && preferredStrategy[scope] !== ordered[j].name) {
              preferredStrategy[scope] = ordered[j].name;
              if (!strategyLogged[scope]) {
                strategyLogged[scope] = true;
                log('wechat-adapter: ' + scope + ' 代码包读取路径形式 = ' + ordered[j].name + '（例：' + candidate + '）');
              }
            }
            if (subjects[s] !== path) mainpackReads += 1;
            return {
              value: value,
              source: ordered[j].name === 'user-data' ? 'cache' : 'package',
              candidate: candidate,
              subject: subjects[s]
            };
          }
          failures.push(ordered[j].name + '(' + candidate + '): 返回空');
        } catch (error) {
          failures.push(ordered[j].name + '(' + candidate + '): ' + hostError(error));
        }
      }
    }
    // 同一路径只详细报一次：远端回落场景下"包内读不到"是**预期**的，逐次刷屏会淹没真问题；
    // 已启用远端时降级为普通日志（真正的失败会在下面"远端下载失败"里报）。
    if (!quiet && !failureLogged[path]) {
      failureLogged[path] = true;
      var where = subjects.length > 1 || subjects[0] !== path ? '（主体 ' + subjects.join(' / ') + '）' : '';
      // 主包侧首屏文件读不到，是分包形态下最致命的一类失败：它发生在**任何分包挂载之前**，
      // 所以与"分包还没下完"无关，却长得一样（同一串 permission denied）。实测踩过：
      // 真机上 mainpack/** 读不到 → 启动就 `pack/project.toml HTTP 404`，日志里却只有一串
      // 路径方言失败。这里直接把"该怎么查"写进日志，省掉一轮来回。
      var hint = '';
      if (subjects[0].indexOf(MAINPACK_DIR) === 0) {
        hint = '；这是**主包侧首屏文件**，与分包挂载无关——该文件没进代码包，' +
          '或 ' + MAINPACK_MANIFEST + ' 与实际产物不一致（发布形态选错/产物是旧的一次）';
      }
      if (remoteConfig) log('wechat-adapter: 包内未命中，转远端 ' + path);
      else warn('wechat-adapter: 读取失败 ' + path + where + ' -> ' + failures.join(' | ') + hint);
    }
    return null;
  }

  function bytesFrom(data) {
    if (!data) return null;
    if (data instanceof ArrayBuffer) return data;
    if (typeof data === 'string') return null;
    if (data.buffer instanceof ArrayBuffer) {
      return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
    }
    return null;
  }

  function readBytesRaw(fs, candidate) {
    return bytesFrom(fs.readFileSync(candidate));
  }

  // 部分基础库/宿主对代码包文件的第二个 encoding 参数支持不一致：先按 utf8 文本读，
  // 读不到再按原始字节读并自行解码。
  function readTextRaw(fs, candidate) {
    var text = null;
    try { text = fs.readFileSync(candidate, 'utf8'); } catch (error) { text = null; }
    if (typeof text === 'string') return text;
    var bytes = bytesFrom(text) || readBytesRaw(fs, candidate);
    return bytes ? decodeUtf8(bytes) : null;
  }

  function decodeUtf8(buffer) {
    var bytes = new Uint8Array(buffer);
    if (typeof TextDecoder === 'function') {
      try { return new TextDecoder('utf-8').decode(bytes); } catch (error) { /* 落到手写解码 */ }
    }
    var out = '';
    for (var i = 0; i < bytes.length;) {
      var b = bytes[i];
      if (b < 0x80) { out += String.fromCharCode(b); i += 1; }
      else if (b >= 0xc0 && b < 0xe0 && i + 1 < bytes.length) {
        out += String.fromCharCode(((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f)); i += 2;
      }
      else if (b >= 0xe0 && b < 0xf0 && i + 2 < bytes.length) {
        out += String.fromCharCode(((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f)); i += 3;
      }
      else if (b >= 0xf0 && i + 3 < bytes.length) {
        var cp = ((b & 0x07) << 18) | ((bytes[i + 1] & 0x3f) << 12) | ((bytes[i + 2] & 0x3f) << 6) | (bytes[i + 3] & 0x3f);
        cp -= 0x10000;
        out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff)); i += 4;
      }
      else { out += '\ufffd'; i += 1; }
    }
    return out;
  }

  function readText(path, quiet) {
    var result = readWithStrategies(path, readTextRaw, quiet);
    noteRead(path, result);
    return result ? result.value : null;
  }

  // 二进制读取：`Response.arrayBuffer()` 必须真的按字节读文件——不能从 utf8 文本反推，
  // 那会把非文本字节（PNG 等）静默替换成 U+FFFD。
  function readBytes(path) {
    var result = readWithStrategies(path, readBytesRaw);
    noteRead(path, result);
    return result ? result.value : null;
  }

  // 资源来源统计：用于证明"素材到底来自包内还是远端"（证据可查，而不是只看启动成功）。
  var assetStats = {
    package: 0, cache: 0, remote: 0, remoteBytes: 0, remoteFailed: 0, verified: 0, checksumMismatch: 0,
    // `checksumUnverified`：**下载了但没有拿清单校验**的次数。第 49 轮实测：一次启动 180 次远端下载里
    // 有 4 次走的是这条放行路径，其中就有被翻转一个字节的 `pack/project.toml`（脏字节被写进缓存、
    // 被当成好文件继续用），而当时既不计数也不告警——`checksumMismatch` 只记"校验了但内容不符"，
    // 不记"压根没校验"。所以这个数必须单独存在，否则整类问题在读数上不可见。
    checksumUnverified: 0, unverifiedSamples: [],
    // 贴图成败必须能自证：ok/fail 由宿主的 onload/onerror 计数，未供应 = 我们明知包内没有、
    // 远端也没拿到就交给宿主（必然失败）。samples 只留前几条，方便在现场日志里直接看到路径。
    imageLoaded: 0, imageFailed: 0, imageMissing: 0, imageCacheLoadFailed: 0, imageFailureSamples: []
  };

  function noteRead(path, result) {
    if (!result) return;
    // 只统计**素材**读取（pack/ 下）：主包文件（如 remote-assets.json）不算资源来源。
    if (scopeOf(path) !== 'pack') return;
    if (result.source === 'cache') assetStats.cache += 1;
    else assetStats.package += 1;
    updateAssetMarkers();
  }

  // 资源读取自检：区分"路径形式不对"和"文件没进包"。
  // 在开发者工具里只需看这一行日志即可定位（这些文件都由发布脚本保证存在）。
  // 刻意不探测 `pack/game.js`：去掉 pack/ 前缀后就是主包入口 game.js，会把"读到别的文件"
  // 误报成 ok——这里改用三个在包根不可能与主包同名的元数据文件，并校验内容形状。
  // 分包形态下这条禁令更强：那时 pack/game.js 是**真的存在**的分包入口（不再是假想中的冲突），
  // 而 subpackage-relative 方言依然会把它读成主包入口，误报会更隐蔽（读到的是合法 JS，形状检查也拦不住）。
  //
  // 分包形态下这三个探针都走主包（它们在首屏清单里，主体是 mainpack/），所以这条自检验证的是
  // **路由层本身**：主包侧可读 = 首屏可读。日志里用 @mainpack 标出主体，避免把"读到了主包那份"
  // 误读成"分包已经就绪"。
  function looksLikeToml(text) {
    return typeof text === 'string' && /(^|\n)\s*(\[|[A-Za-z_][\w.-]*\s*=)/.test(text);
  }

  function probePackReadability() {
    var probes = [
      { path: PACK_PREFIX + 'manifest.txt', shape: function (t) { return typeof t === 'string' && t.length > 0; } },
      { path: PACK_PREFIX + 'project.toml', shape: looksLikeToml }
    ];
    var report = [];
    for (var i = 0; i < probes.length; i++) {
      var name = probes[i].path.slice(PACK_PREFIX.length);
      var result = readWithStrategies(probes[i].path, readTextRaw, true);
      // 主体不是逻辑路径本身 = 读的是主包 mainpack/ 那一份（分包形态）。
      var via = (result && result.subject !== probes[i].path) ? '@' + MAINPACK_DIR.replace(/\/$/, '') : '';
      if (!result) report.push(name + '=FAIL');
      else if (!probes[i].shape(result.value)) report.push(name + '=WRONG-CONTENT');
      else report.push(name + '=' + (result.source === 'cache' ? 'cache' : 'ok') + via);
    }
    log('wechat-adapter: 资源读取自检 ' + report.join(' ') + '（路径形式: main=' + (preferredStrategy.main || 'none') +
      ' pack=' + (preferredStrategy.pack || 'none') +
      (remoteConfig ? '，远端回落已启用 ' + remoteConfig.packBaseUrl : '') +
      '，分包状态: ' + packState + (packState === 'loading' ? '(' + packProgress + '%)' : '') + '）');
    return report.join(' ');
  }

  // ---------------------------------------------------------------- 远端资源回落
  // 形态：**包内优先，读不到才走远端**。远端下载后落地到 USER_DATA_PATH（即上面策略表里的
  // `user-data`），因此后续读取天然命中缓存，不必再建一套缓存索引。
  //
  // 边界（别把本地跑通当成 CDN 就绪）：本地服务只能验证下载/缓存/校验/回落这些**客户端**逻辑；
  // 真机的 HTTPS 证书、mp 后台 downloadFile 合法域名、CDN 缓存与限速都测不到。
  var REMOTE_CONFIG_FILE = 'remote-assets.json';
  var remoteConfig = null;
  var remoteConfigLoaded = false;
  var manifestChecksums = null;
  var checksumLookupFailed = false;
  var inFlightDownloads = {};
  var manifestDownload = null;

  function loadRemoteConfig() {
    if (remoteConfigLoaded) return remoteConfig;
    remoteConfigLoaded = true;
    // quiet：这个文件**可以不存在**（包内完整资源时就没有它），四种路径形式全空是常态，
    // 不该在控制台刷一行"读取失败"——真要有远端配置却读不到，下面启用失败会另报。
    var text = null;
    try { text = readText(REMOTE_CONFIG_FILE, true); } catch (error) { text = null; }
    if (!text) return null;
    var parsed = null;
    try { parsed = JSON.parse(text); } catch (error) {
      warn('wechat-adapter: ' + REMOTE_CONFIG_FILE + ' JSON 解析失败 -> ' + hostError(error));
      return null;
    }
    if (!parsed || typeof parsed.packBaseUrl !== 'string' || !/^https?:\/\//.test(parsed.packBaseUrl)) {
      warn('wechat-adapter: ' + REMOTE_CONFIG_FILE + ' 缺少合法的 packBaseUrl（需以 http(s):// 开头）');
      return null;
    }
    remoteConfig = {
      packBaseUrl: parsed.packBaseUrl.charAt(parsed.packBaseUrl.length - 1) === '/'
        ? parsed.packBaseUrl
        : parsed.packBaseUrl + '/',
      timeoutMs: typeof parsed.timeoutMs === 'number' && parsed.timeoutMs > 0 ? parsed.timeoutMs : 15000,
      verifyChecksum: parsed.verifyChecksum !== false
    };
    log('wechat-adapter: 远端资源回落已启用 base=' + remoteConfig.packBaseUrl +
      ' verifyChecksum=' + remoteConfig.verifyChecksum);
    return remoteConfig;
  }

  function remoteUrlFor(path) {
    return remoteConfig.packBaseUrl + path.slice(PACK_PREFIX.length);
  }

  // 缓存路径 = 用户目录 + **发布标识** + 逻辑路径。
  //
  // 为什么必须带发布标识（修的是"发布新版本后旧缓存永久遮蔽新内容"）：
  // 缓存键原本只有逻辑路径（`user-data/pack/scripts/x.lua`），而 `readWithStrategies` 里
  // user-data 是**第一候选**、命中即返回 —— 校验只在**新下载**时发生。于是同一个文件名
  // 在发布新版本后仍然命中上一版的缓存：改了 Lua、换了贴图，真机上还是老样子，而且不会
  // 自愈（玩家不清缓存就永远修不好）。把发布标识编进目录名，这一版一变整批路径就变，
  // 旧缓存自然失效（并且能整目录清掉，见 pruneStaleCacheRoot）。
  //
  // 为什么用**发布级**而不是**逐文件**标识：逐文件的目录名会让一次发布把文件散进几十个
  // `v-<hash>/` 里，"清掉上一版"就没有可删的单元了（只剩遍历删单文件）。
  // 清单（`相对路径 size 校验和`）本身逐行列了这批资源的全部内容，哈希它就等价于
  // "这批资源变了没有"。
  //
  // 清单不可得时（老产物/清单没打进包）返回旧的不带标识路径：宁可回到旧行为，
  // 也不要因为"算不出键"而把整批缓存判成未命中、每次启动重下全部素材。
  function cachePathFor(path) {
    if (!userRoot) return null;
    // 清单**永远**走不带标识的路径：它必须在"还不知道标识"的时候就能读写。
    // 远端形态下 ensureManifest 的顺序是"先下载并落盘清单 → 再据它算标识"，若清单也带标识，
    // 落盘时就会用**上一版**清单算出的键（那时新清单还没读进来），于是新清单被写进旧键目录、
    // 下一轮又读回旧清单——标识永远停在上一版，新内容永远不会被下载（比"旧缓存遮蔽"更硬）。
    if (path === PACK_PREFIX + 'manifest.txt') return userRoot + '/' + path;
    var key = releaseKey();
    if (!key) return userRoot + '/' + path;
    return userRoot + '/v-' + key + '/' + path;
  }

  var releaseKeyMemo = null;
  var releaseKeyResolved = false;

  function releaseKey() {
    if (releaseKeyResolved) return releaseKeyMemo;
    releaseKeyResolved = true;
    var manifest = loadManifestChecksums();
    if (!manifest || !manifestText) return null;
    releaseKeyMemo = fnv1a64Text(manifestText);
    pruneStaleCacheRoot(userRoot + '/v-' + releaseKeyMemo);
    return releaseKeyMemo;
  }

  // 新清单落地后必须把**由旧清单派生的一切**作废（校验表、清单原文、发布标识）。
  // 不作废的后果不是"稍微不准"，而是发布标识永远停在上一版：所有资源继续按旧键读写，
  // 新内容永远不会被下载。失败标记也一并清掉——那可能是"清单还没下下来"时的误判。
  function resetManifestDerived() {
    manifestChecksums = null;
    manifestText = null;
    checksumLookupFailed = false;
    releaseKeyMemo = null;
    releaseKeyResolved = false;
  }

  // 上一版目录不再被任何读取命中，但它**仍然占着用户目录配额**（远端形态下那可能是
  // 几 MB 的贴图与音频）。每次启动按"本次用到的发布标识"清一次旧的，把占用收敛到
  // 一代以内，而不是让每一版都永久留在盘上。
  //
  // 只删**我们自己写下的那个目录**（storage 里的标记）：绝不按 `v-*` 通配去猜，
  // 免得把玩家目录里别的东西当垃圾删掉。
  var CACHE_ROOT_KEY = 'rustlua_asset_cache_root';
  var prunedRoot = null;

  function pruneStaleCacheRoot(currentRoot) {
    if (!currentRoot || prunedRoot === currentRoot) return;
    prunedRoot = currentRoot;
    var previous = safeCall(wx, 'getStorageSync', CACHE_ROOT_KEY);
    var fs = fsManager();
    if (previous && typeof previous === 'string' && previous !== currentRoot
      && previous.indexOf(userRoot + '/v-') === 0 && fs && typeof fs.rmdirSync === 'function') {
      try {
        fs.rmdirSync(previous, true);
        log('wechat-adapter: 已清理上一版资源缓存 ' + previous);
      } catch (error) {
        warn('wechat-adapter: 清理上一版资源缓存失败 ' + previous + ' -> ' + hostError(error));
      }
    }
    try { if (typeof wx.setStorageSync === 'function') wx.setStorageSync(CACHE_ROOT_KEY, currentRoot); } catch (error) { /* 存不下就算了 */ }
  }

  // FNV-1a 64：与 assetc 的清单校验保持一致（清单里是 16 位十六进制）。
  // 用两个 32 位半字手算，避免依赖 BigInt（部分宿主/基础库版本没有）。
  // hash = hi·2³² + lo，乘以 P = 0x100000001b3 = 0x100·2³² + 0x1b3 时：
  //   lo' = lo·0x1b3 的低 32 位
  //   hi' = (lo·0x1b3 的高位 + hi·0x1b3 + lo·0x100) mod 2³²
  // 注意：不能把两个半字拼回 Number（hi·2³² 远超 2⁵³，精度会丢），必须按 8+8 位十六进制拼接。
  function pad8(value) {
    var text = (value >>> 0).toString(16);
    while (text.length < 8) text = '0' + text;
    return text;
  }

  function fnv1a64Hex(buffer) {
    var bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    var lo = 0x84222325, hi = 0xcbf29ce4; // 0xcbf29ce484222325
    for (var i = 0; i < bytes.length; i++) {
      lo = (lo ^ bytes[i]) >>> 0;
      var m = lo * 435;                        // lo × 0x1b3
      var mHi = Math.floor(m / 4294967296);
      var mLo = m % 4294967296;
      var cross = hi * 435 + lo * 256;         // hi × 0x1b3 + lo × 0x100
      var cLo = cross % 4294967296;
      hi = (mHi + cLo) % 4294967296;
      lo = mLo;
    }
    return pad8(hi) + pad8(lo);
  }

  // 文本 → 同口径的 64 位指纹。只用于**缓存命名空间**（"清单变没变"），与 assetc 的
  // 字节级校验无关，所以按 UTF-16 码元拆字节即可，不必做真正的 UTF-8 编码。
  function fnv1a64Text(text) {
    var bytes = new Uint8Array(text.length * 2);
    for (var i = 0; i < text.length; i++) {
      var code = text.charCodeAt(i);
      bytes[i * 2] = code & 0xff;
      bytes[i * 2 + 1] = (code >>> 8) & 0xff;
    }
    return fnv1a64Hex(bytes);
  }

  // 清单：`相对路径\tsize\tfnv1a(16 位十六进制)`，首行是 assetc_manifest。
  //
  // `checksumLoading` 是**重入保护**，不是优化：这个函数自己要通过 readText 读
  // `pack/manifest.txt`，而读取的候选里包含用户目录缓存 —— 缓存路径又要先算出发布标识，
  // 而发布标识来自这份清单。不设这道闸就是无限递归（读清单 → 算缓存路径 → 读清单）。
  // 加载期间 `releaseKey` 返回 null，清单自己走**不带标识**的路径。
  var checksumLoading = false;
  var manifestText = null;
  function loadManifestChecksums() {
    if (manifestChecksums || checksumLookupFailed) return manifestChecksums;
    if (checksumLoading) return null;
    checksumLoading = true;
    try {
      var text = readText(PACK_PREFIX + 'manifest.txt');
      if (!text) {
        // 远端形态下"现在读不到清单"不等于"永远没有清单"：ensureManifest 随后会把它下下来。
        // 在这里 latch 住会让 ensureManifest 直接短路返回 —— 清单永远不下载、校验永远无据可依、
        // 发布标识永远是空（每次都按旧路径读写）。只有"根本不会有远端"时才记这次失败。
        //
        // **第 51 轮修正**：判"根本不会有远端"不能只看 `!remoteConfig` —— 远端配置是**惰性**加载的，
        // 启动自检与缓存路径解析都会在它加载**之前**先读一次清单，那一刻 `!remoteConfig` 为真但并不是
        // "没有远端"。按 `!remoteConfig` latch 会形成**自锁**：`ensureManifest()` 见标记即直接返回
        // （实测读数 `manifest=0 lookupFailed=1 download=0 loading=0`）⇒ 清单永远不下载 ⇒ 而唯一能清这个
        // 标记的 `resetManifestDerived()` 只在**清单下载完成**时调用 ⇒ 标记永远为真 ⇒ 该会话里所有远端口
        // 资源都无据可依（实测首屏三个文件被静默放行，其中就有被翻转一个字节的 `pack/project.toml`）。
        // 正确判据是"配置**已经加载过**且确实为空"：`remoteConfigLoaded && !remoteConfig`。
        if (remoteConfigLoaded && !remoteConfig) checksumLookupFailed = true;
        return null;
      }
      var map = {};
      var lines = text.split('\n');
      for (var i = 0; i < lines.length; i++) {
        var parts = lines[i].replace(/\r$/, '').split('\t');
        if (parts.length < 3) continue;
        var size = parseInt(parts[1], 10);
        if (isNaN(size)) continue;
        map[parts[0]] = { size: size, checksum: parts[2] };
      }
      manifestText = text;
      manifestChecksums = map;
      return map;
    } finally {
      checksumLoading = false;
    }
  }

  // 记一笔"没校验就放行"。**必须可见**：第 49 轮实测这条路静默吃掉了 4/180 次下载（含被篡改的
  // `pack/project.toml`），而当时日志里连一句"未校验"都没有——`verifyDownloaded` 上面那句注释
  // 承诺过"下面会记一笔'未校验'"，实际并不存在。samples 只留前几条，避免刷屏。
  function noteUnverified(path, reason) {
    assetStats.checksumUnverified += 1;
    // 第 51 轮：把"为什么清单不可得"直接量出来。`loadManifestChecksums()` 与 `ensureManifest()` 都会在
    // 标志为真时**直接返回不下载**（前者靠 checksumLookupFailed，后者也靠它），所以"清单未就绪"至少有
    // 三种互斥成因：①压根没下载过（D=0）、②正在下载中（D=1）、③被失败标记短路（F=1）。
    // 三者对应的修法完全不同，日志里必须能区分，不能只写"未就绪"。
    var flags = 'manifest=' + (manifestChecksums ? 1 : 0) + ' lookupFailed=' + (checksumLookupFailed ? 1 : 0)
      + ' download=' + (manifestDownload ? 1 : 0) + ' loading=' + (checksumLoading ? 1 : 0)
      + ' configLoaded=' + (remoteConfigLoaded ? 1 : 0);
    var detailed = reason + '［' + flags + '］';
    if (assetStats.unverifiedSamples.length < 8) assetStats.unverifiedSamples.push(path + ':' + detailed);
    warn('wechat-adapter: 未校验就放行 ' + path + '（' + detailed + '）');
  }

  function verifyDownloaded(path, buffer) {
    if (!remoteConfig.verifyChecksum) return true;
    var manifest = loadManifestChecksums();
    if (!manifest) {
      noteUnverified(path, '清单未就绪');
      return true;
    }
    var entry = manifest[path.slice(PACK_PREFIX.length)];
    if (!entry) {
      noteUnverified(path, '清单里没有这一项');
      return true;
    }
    var bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    if (bytes.length !== entry.size) {
      assetStats.checksumMismatch += 1;
      warn('wechat-adapter: ' + path + ' 大小不符（远端 ' + bytes.length + ' / 清单 ' + entry.size + '）');
      return false;
    }
    var actual = fnv1a64Hex(bytes);
    if (actual !== entry.checksum) {
      assetStats.checksumMismatch += 1;
      warn('wechat-adapter: ' + path + ' 校验和不符（远端 ' + actual + ' / 清单 ' + entry.checksum + '）');
      return false;
    }
    assetStats.verified += 1;
    return true;
  }

  function ensureCacheDir(path) {
    var fs = fsManager();
    if (!fs || typeof fs.mkdirSync !== 'function') return;
    var absolute = cachePathFor(path);
    if (!absolute) return;
    var directory = absolute.slice(0, absolute.lastIndexOf('/'));
    try { fs.mkdirSync(directory, true); return; } catch (error) { /* 递归参数不被支持时逐级兜底 */ }
    var segments = directory.split('/');
    var built = '';
    for (var i = 0; i < segments.length; i++) {
      built = built ? built + '/' + segments[i] : segments[i];
      try { fs.mkdirSync(built); } catch (error) { /* 已存在 */ }
    }
  }

  // 清掉缓存里的同名文件（校验失败时用）。
  function removeCache(path) {
    var fs = fsManager();
    var absolute = cachePathFor(path);
    if (!fs || !absolute || typeof fs.unlinkSync !== 'function') return;
    try { fs.unlinkSync(absolute); } catch (error) { /* 不存在或不可删都无所谓 */ }
  }

  function writeCache(path, buffer) {
    var fs = fsManager();
    if (!fs || typeof fs.writeFileSync !== 'function') return false;
    var absolute = cachePathFor(path);
    if (!absolute) return false;
    ensureCacheDir(path);
    try {
      fs.writeFileSync(absolute, buffer instanceof Uint8Array ? buffer.buffer : buffer);
      return true;
    } catch (error) {
      warn('wechat-adapter: 写入缓存失败 ' + absolute + ' -> ' + hostError(error));
      return false;
    }
  }

  function downloadOnce(url) {
    return new Promise(function (resolve, reject) {
      var settled = false;
      var task = null;
      var timer = setTimeout(function () {
        if (settled) return;
        settled = true;
        if (task && typeof task.abort === 'function') { try { task.abort(); } catch (error) { /* ignore */ } }
        reject(new Error('下载超时（' + remoteConfig.timeoutMs + 'ms）'));
      }, remoteConfig.timeoutMs);
      function finish(error, value) {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(error); else resolve(value);
      }

      if (typeof wx.downloadFile === 'function') {
        try {
          // **不传 filePath**：让宿主下到临时文件，校验通过后才由我们写进缓存。
          // 传 filePath 会让宿主直接把字节落到缓存路径——校验失败时"已丢弃"的脏文件
          // 仍然躺在缓存里，后续读取会命中它（实测被反向用例抓到：篡改后的
          // project.toml 被拒绝，但启动依然成功，因为缓存里已经有那份脏数据）。
          task = wx.downloadFile({
            url: url,
            success: function (result) {
              var status = result && result.statusCode;
              if (status && status !== 200) { finish(new Error('HTTP ' + status)); return; }
              var target = (result && (result.tempFilePath || result.filePath)) || null;
              if (!target) { finish(new Error('下载成功但没有临时文件路径')); return; }
              finish(null, target);
            },
            fail: function (error) { finish(new Error(hostError(error))); }
          });
          return;
        } catch (error) { finish(new Error(hostError(error))); return; }
      }
      if (typeof wx.request === 'function') {
        try {
          task = wx.request({
            url: url,
            responseType: 'arraybuffer',
            success: function (result) {
              var status = result && result.statusCode;
              if (status && status !== 200) { finish(new Error('HTTP ' + status)); return; }
              finish(null, result && result.data);
            },
            fail: function (error) { finish(new Error(hostError(error))); }
          });
          return;
        } catch (error) { finish(new Error(hostError(error))); return; }
      }
      finish(new Error('宿主既没有 wx.downloadFile 也没有 wx.request'));
    });
  }

  function toUint8(bytes) {
    if (!bytes) return null;
    return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  }

  // 统一返回 Uint8Array：ArrayBuffer 只有 byteLength 没有 length，混用会让日志/统计出现 undefined。
  function readHostFile(target) {
    var fs = fsManager();
    if (!fs || target === null || target === undefined) return null;
    if (target instanceof ArrayBuffer) return new Uint8Array(target);
    if (typeof target !== 'string') return toUint8(bytesFrom(target));
    try { return toUint8(bytesFrom(fs.readFileSync(target))); } catch (error) { return null; }
  }

  // 下载 → 校验 → 落地缓存。`verify=false` 只用于清单自身（它没有可对照的清单项）。
  // 同一路径并发只下载一次。
  function downloadToCache(path, verify) {
    var url = remoteUrlFor(path);
    // 第 50 轮插桩：`verify=false` 按设计只该出现在**清单自己**身上（它没有可对照的清单项）。
    // 非清单文件走到这里，说明调用方压根没请求校验（第 49 轮实测的形态：`pack/project.toml`
    // 在 `pack/manifest.txt` **之前**就被下下来了）——那种情况下 `verifyDownloaded` 根本不会被调用，
    // 所以只在它里面计数是不够的，这里必须也报一笔，才能指认"是哪条路绕过了校验"。
    if (!verify && remoteConfig && remoteConfig.verifyChecksum && path !== PACK_PREFIX + 'manifest.txt') {
      noteUnverified(path, '这条下载路径没有请求校验');
    }
    return downloadOnce(url).then(function (target) {
      var bytes = readHostFile(target);
      if (!bytes) {
        assetStats.remoteFailed += 1;
        warn('wechat-adapter: 远端下载后读不回字节 ' + url);
        return false;
      }
      if (verify && !verifyDownloaded(path, bytes)) {
        assetStats.remoteFailed += 1;
        // 双保险：万一缓存路径上残留了同名脏文件（例如旧实现用 filePath 让宿主直接落盘），
        // 一定要清掉——否则"已丢弃"只是口头上的，后续读取照样会命中它。
        removeCache(path);
        warn('wechat-adapter: 远端文件未通过清单校验，已丢弃 ' + path);
        return false;
      }
      if (!writeCache(path, bytes)) {
        assetStats.remoteFailed += 1;
        warn('wechat-adapter: 远端下载成功但无法写入缓存 ' + path + '（本轮仍会用它，下次会重新下载）');
      }
      // 清单换了就是把整批资源换了：由它派生的校验表与发布标识必须立刻作废重算
      // （否则发布标识停在上一版，后面所有资源继续按旧键读写）。
      if (path === PACK_PREFIX + 'manifest.txt') resetManifestDerived();
      assetStats.remote += 1;
      assetStats.remoteBytes += bytes.length;
      updateAssetMarkers();
      log('wechat-adapter: 远端取得 ' + path + ' (' + bytes.length + 'B) <- ' + url);
      return true;
    }).catch(function (error) {
      assetStats.remoteFailed += 1;
      warn('wechat-adapter: 远端下载失败 ' + url + ' -> ' + hostError(error));
      return false;
    });
  }

  // 清单必须先落地，否则后面的文件"校验"实际是无据可依（会静默放过被篡改的内容）。
  function ensureManifest() {
    if (manifestChecksums || checksumLookupFailed) return Promise.resolve(manifestChecksums);
    if (!remoteConfig) return Promise.resolve(null);
    if (!manifestDownload) {
      manifestDownload = downloadToCache(PACK_PREFIX + 'manifest.txt', false).then(function (ok) {
        if (!ok) checksumLookupFailed = true;
        return loadManifestChecksums();
      });
    }
    return manifestDownload;
  }

  function fetchRemote(path) {
    if (!remoteConfig) return Promise.resolve(false);
    if (inFlightDownloads[path]) return inFlightDownloads[path];
    var isManifest = path === PACK_PREFIX + 'manifest.txt';
    var prepare = (remoteConfig.verifyChecksum && !isManifest)
      ? ensureManifest()
      : Promise.resolve(null);
    var promise = prepare.then(function () {
      return downloadToCache(path, remoteConfig.verifyChecksum && !isManifest);
    }).then(function (ok) {
      delete inFlightDownloads[path];
      return ok;
    });
    inFlightDownloads[path] = promise;
    return promise;
  }

  function updateAssetMarkers() {
    bodyElement.setAttribute('data-rustlua-asset-package', String(assetStats.package));
    bodyElement.setAttribute('data-rustlua-asset-cache', String(assetStats.cache));
    bodyElement.setAttribute('data-rustlua-asset-remote', String(assetStats.remote));
    bodyElement.setAttribute('data-rustlua-asset-remote-bytes', String(assetStats.remoteBytes));
    bodyElement.setAttribute('data-rustlua-asset-verified', String(assetStats.verified));
    bodyElement.setAttribute('data-rustlua-asset-source', assetStats.remote > 0
      ? (assetStats.package + assetStats.cache > 0 ? 'mixed' : 'remote')
      : 'package');
  }

  // ---------------------------------------------------------------- pack/ 资源载入

  var packReady = null;
  // 分包载入状态机：idle（还没试）| loading | ready（已挂载）| failed | absent（game.json 没声明）。
  // 显式记状态而不是只看 Promise，是因为三件事都要读它：进度条要不要画（loading）、
  // 失败横幅要不要画（failed）、以及失败后要不要再下一次（冷却，见 ensurePack）。
  var packState = 'idle';
  var packProgress = 0;
  var packProgressEvents = 0;
  var packMountFrame = -1;
  // 调用过几次 wx.loadSubpackage。诊断里与 declared 一起看：**声明了却一次都没调**是最危险的一种
  // 失败（分包永远不会就绪，而失败通道只挂在"挂载失败"上，屏幕上不会有任何提示）。
  var packMountAttempts = 0;
  var packFailure = '';
  var packRetryCooldownUntil = 0;
  // 失败后的重试冷却：不设的话每一次 pack/ 读取都会重新下一次包 → 重试风暴（真机上表现为反复下载）。
  // 10 秒是"玩家从标题进到玩法场景"的量级：够短（网络恢复后能自己好），够长（不会刷屏下载）。
  var PACK_RETRY_COOLDOWN_MS = 10000;
  // 冷却可现场覆盖（`wx.setStorageSync('rustlua_pack_retry_cooldown_ms', 0)`）：与启动看门狗同理，
  // 否则"失败后能自愈"这条路径没法被自动化验证（等 10 秒的用例进不了质量门）。
  var PACK_RETRY_COOLDOWN_KEY = 'rustlua_pack_retry_cooldown_ms';

  function packRetryCooldownMs() {
    var value = parseInt(safeCall(wx, 'getStorageSync', PACK_RETRY_COOLDOWN_KEY), 10);
    return value >= 0 ? value : PACK_RETRY_COOLDOWN_MS;
  }
  // 宿主既不 success 也不 fail 时的兜底：不能让 Promise 永远悬着（等它的资源读取会跟着永远挂住）。
  var PACK_LOAD_TIMEOUT_MS = 20000;

  // `pack/` 是不是**分包**、以及它叫什么名字 —— 判据必须是**我们自己打进包里的数据**，
  // **不能读 `game.json`**。
  //
  // 实测（真机/开发者工具同一形态，2026-09-16）：宿主对 `readFileSync('game.json')` 一律拒绝——
  //   user-data:  http://usr/game.json … no such file or directory
  //   root-relative / leading-slash / dot-prefix: readFileSync:fail permission denied
  // 它是**平台配置**，宿主只在自己解析时用，不向 FileSystemManager 开放（与小程序侧 `app.json`
  // 同理）。而这条读取原本是"要不要 wx.loadSubpackage"的**唯一**判据，于是分包形态会在宿主上
  // **静默退化**：判定成"没声明分包" → 一次 loadSubpackage 都不调 → pack/ 永远读不到 →
  // 进玩法场景缺素材。并且**一声不响**：失败通道挂在"挂载失败"上，这里压根没去挂载。
  //
  // 现在的判据是首屏清单 `mainpack.json` 的 `layout.subpackage_root`：同一份清单既回答
  // "这个逻辑路径读主包还是读分包"，也回答"要不要挂分包"，两者同源就不会互相矛盾；
  // 而且它本来就在启动路径上被读（已实测可读）。没有清单（full / remote 形态）⇒ 不是分包
  // ⇒ 不挂载，也就没有假失败噪音。
  function packSubpackageName() {
    var manifest = firstScreenKeys();
    var layout = manifest && manifest.layout;
    var name = layout && layout.subpackage_root;
    return name ? String(name).replace(/\/$/, '') : '';
  }

  function hasPackSubpackage() {
    return packSubpackageName() !== '';
  }

  // wasm 内核这次到底在**主包**还是在**分包**——启动路径上三处都要用到这个判据
  // （要不要等分包、屏上写的是哪句话、实例化传哪条路径），所以只在这里算一次。
  // 判据跟着形态而不是跟着"文件此刻在不在"：分包未挂载时 `pack/game_web_bg.wasm`
  // 读不到，那正是"要等分包"的信号，不是"退回主包路径"的理由（主包里根本没有它）。
  function isWasmInPack() {
    return hasPackSubpackage();
  }

  function wasmModulePath() {
    return isWasmInPack() ? WASM_MODULE_PATH_PACK : WASM_MODULE_PATH_MAIN;
  }

  // 进度回调只服务于"给玩家看见还在下"：真机上分包是几十 MB 量级的下载，微信端却**没有**
  // 任何 loading UI（statusElement 是个 no-op 对象），所以这条进度是我们唯一的可见证据。
  function attachPackProgress(task) {
    if (!task || typeof task.onProgressUpdate !== 'function') return false;
    try {
      task.onProgressUpdate(function (res) {
        packProgressEvents += 1;
        var value = res && typeof res.progress === 'number' ? res.progress : 0;
        packProgress = Math.max(0, Math.min(100, Math.round(value)));
      });
      return true;
    } catch (error) {
      warn('wechat-adapter: 分包进度回调挂不上 -> ' + hostError(error));
      return false;
    }
  }

  // `pack/`（声明为分包时）按需载入；多次调用共享同一个 Promise（失败不缓存，冷却后允许重试）。
  //
  // **绝不 reject**：加载失败一律 resolve(false) 并走可见的失败通道（console.error +
  // data-rustlua-pack=failed + 屏幕红横幅）。理由：调用方既有 `fetchShim`（它要的是"能不能读"）
  // 也有 `start()`（它代表**整局启动**）。让一个分包下载失败去 reject 启动 Promise，结果是
  // 标题画面已经画出来了却被整屏错误页盖掉——黑屏、且比"分包没挂上"更难排查。
  function ensurePack() {
    if (packState === 'ready') return Promise.resolve(true);
    if (packState === 'absent') return Promise.resolve(false);
    if (packReady) return packReady;
    // 未声明分包（清单里没有 subpackage_root）：不需要也不应该 loadSubpackage，按普通目录读取。
    if (!hasPackSubpackage()) {
      packState = 'absent';
      log('wechat-adapter: 本包未声明 ' + PACK_SUBPACKAGE + ' 分包（pack/ 按主包内普通目录读取，与远端回落/包内完整两种形态一致）');
      return Promise.resolve(false);
    }
    var mountName = packSubpackageName() || PACK_SUBPACKAGE;
    if (typeof wx.loadSubpackage !== 'function') {
      warn('wechat-adapter: 宿主没有 wx.loadSubpackage，但首屏清单声明了 ' + mountName + ' 分包');
      packState = 'absent';
      return Promise.resolve(false);
    }
    if (packState === 'failed' && Date.now() < packRetryCooldownUntil) return Promise.resolve(false);
    packState = 'loading';
    packReady = new Promise(function (resolve) {
      var settled = false;
      function finish(ok, err) {
        if (settled) return;
        settled = true;
        if (ok) {
          packState = 'ready';
          packMountFrame = frameCount;
          // 挂载成功的瞬间个别宿主还没把文件挂上：开一个短重试窗口（见 readTextWithRetry）。
          packRetryDeadline = Date.now() + PACK_RETRY_WINDOW_MS;
          log('wechat-adapter: ' + mountName + ' 分包已挂载（第 ' + frameCount + ' 帧，进度回调 ' + packProgressEvents + ' 次）');
          resolve(true);
          return;
        }
        packReady = null;
        packState = 'failed';
        packRetryCooldownUntil = Date.now() + packRetryCooldownMs();
        packFailure = mountName + ' 分包载入失败: ' + ((err && err.errMsg) || err);
        // 失败必须**响亮**，但**不能**掀翻启动 Promise（见函数头注释）。三条通道同时给：
        // 控制台（开发者工具）、属性（无头 harness/自动化）、屏幕横幅（真机上唯一可用的那条）。
        console.error('[wechat-adapter] ' + packFailure + ' 现场：' + JSON.stringify(diagSnapshot()));
        bodyElement.setAttribute('data-rustlua-pack', 'failed');
        bodyElement.setAttribute('data-rustlua-pack-detail', packFailure.slice(0, 200));
        publishDiag();
        resolve(false);
      }
      var task = null;
      try {
        packMountAttempts += 1;
        task = wx.loadSubpackage({
          name: mountName,
          success: function () { finish(true, null); },
          fail: function (err) { finish(false, err); }
        });
      } catch (error) {
        finish(false, error);
        return;
      }
      attachPackProgress(task);
      setTimeout(function () {
        finish(false, { errMsg: 'loadSubpackage 无回调超时（' + PACK_LOAD_TIMEOUT_MS + 'ms）' });
      }, PACK_LOAD_TIMEOUT_MS);
    });
    return packReady;
  }

  // boot 之前的挂载重试（只有"内核在分包里"这一种形态会走到）。
  //
  // **为什么现在必须有**：内核在主包时，一次分包下载失败的代价是"玩法场景缺素材"，标题照常出、
  // 玩家甚至没感觉；内核搬进 pack/ 之后，同一次失败的代价变成"这一次根本进不去"。瞬时网络抖动
  // 不该把整局启动打死，所以在实例化之前自动再挂一次。
  // 上限 2 次尝试（= 1 次自动重试）：每次尝试自带 `PACK_LOAD_TIMEOUT_MS`（20 秒）无回调兜底，
  // 再加一轮就是一分钟起步的进度条。**本机证明不了第二次重试救回的概率**（假包要么挂上要么挂不上，
  // 没有真机的抖动分布），能给的只有"失败不再一次性致命"这条下限。
  //
  // 这里刻意绕过 `packRetryCooldownUntil`：那条 10 秒冷却是**读路径**防重试风暴用的（玩法场景中
  // 连续几十次 pack/ 读取都会撞上它），而 boot 这条链由自己掌握节奏，等满一个短退避就把冷却清零。
  // 冷却清零不影响读路径的语义——那里真正的判据是 `packState`，重试成功后它变回 ready。
  var PACK_BOOT_ATTEMPT_LIMIT = 2;
  var PACK_BOOT_RETRY_DELAY_MS = 1500;
  var packBootRetryNotice = '';

  function ensurePackForBoot(remainingRetries) {
    return ensurePack().then(function (mounted) {
      if (mounted) {
        packBootRetryNotice = '';
        return true;
      }
      // absent = 本包压根没声明分包（宿主没有 loadSubpackage / 清单没有 root）：重试没有意义。
      if (packState === 'absent' || remainingRetries <= 0) {
        packBootRetryNotice = '';
        return false;
      }
      packBootRetryNotice = '分包下载失败，正在重试（还会再试 ' + remainingRetries + ' 次）…';
      return new Promise(function (resolve) {
        setTimeout(resolve, PACK_BOOT_RETRY_DELAY_MS);
      }).then(function () {
        packRetryCooldownUntil = 0;
        return ensurePackForBoot(remainingRetries - 1);
      });
    });
  }

  function normalizePath(url) {
    var path = String(url == null ? '' : url);
    var q = path.indexOf('?');
    if (q >= 0) path = path.slice(0, q);
    var h = path.indexOf('#');
    if (h >= 0) path = path.slice(0, h);
    while (path.indexOf('./') === 0) path = path.slice(2);
    while (path.charAt(0) === '/') path = path.slice(1);
    return path;
  }

  function notFound(path) {
    return {
      ok: false,
      status: 404,
      url: path,
      text: function () { return Promise.resolve(''); },
      arrayBuffer: function () { return Promise.resolve(new ArrayBuffer(0)); }
    };
  }

  // 文本 → 运行时认识的 Response 形状。抽出来是因为它现在有两条调用路径
  // （包内直读、远端下载后回读），两份长得一样的字面量迟早会漂移。
  function toResponse(path, text) {
    if (text === null) return notFound(path);
    return {
      ok: true,
      status: 200,
      url: path,
      text: function () { return Promise.resolve(text); },
      arrayBuffer: function () {
        var bytes = readBytes(path);
        if (bytes) return Promise.resolve(bytes);
        warn('wechat-adapter: 无法按字节读取 ' + path + '，退回文本近似');
        var approx = new Uint8Array(text.length);
        for (var i = 0; i < text.length; i++) approx[i] = text.charCodeAt(i) & 0xff;
        return Promise.resolve(approx.buffer);
      }
    };
  }

  // 运行时的 `fetchText(url)` → `response.ok` / `response.text()`。
  // pack/ 内文件：包内（含缓存）优先，读不到再走远端下载，下载后再读一次（这次读到的是缓存）。
  //
  // **首屏清单内的路径不等分包**：它们的主体是主包 mainpack/，本来就直接可读；去等一个
  // 下载完成等于把首屏白屏时间绑在分包大小上——而"首屏不等分包"正是分包形态存在的理由。
  function fetchShim(url) {
    var path = normalizePath(url);
    var needsPack = path.indexOf(PACK_PREFIX) === 0 && !isFirstScreenPath(path);
    return (needsPack ? ensurePack() : Promise.resolve(false)).then(function () {
      // 分包刚 loadSubpackage 成功时，个别宿主还没把文件挂上：对 pack/ 内文件做几次短重试，
      // 避免把"挂载竞态"误判成"文件不存在"。
      return needsPack ? readTextWithRetry(path) : readText(path);
    }).then(function (text) {
      if (text !== null || !needsPack) return text;
      var config = loadRemoteConfig();
      if (!config) return null;
      return fetchRemote(path).then(function (ok) {
        if (!ok) return null;
        var cached = readText(path);
        if (cached === null) warn('wechat-adapter: 远端已取得 ' + path + ' 但缓存仍读不到');
        return cached;
      });
    }).then(function (text) {
      return toResponse(path, text);
    });
  }

  var PACK_READ_RETRIES = 5;
  var PACK_READ_RETRY_DELAY_MS = 60;
  // 重试只服务于"分包刚 loadSubpackage 成功、文件还没挂上"这个竞态窗口。
  // 早期实现是**每个文件**都重试 5 次（每次 60ms）→ 远端回落场景下每个文件白等 ~240ms，
  // 93 个素材直接把启动拖到 24 秒（实测）。现在改成全局时间窗 + 每路径最多重试一次。
  var PACK_RETRY_WINDOW_MS = 800;
  var packRetryDeadline = 0;

  function readTextWithRetry(path) {
    var attempts = 0;
    function step() {
      // 远端回落已启用时不做重试：包内读不到本来就是预期路径，重试只是浪费时间。
      var inWindow = !remoteConfig && Date.now() < packRetryDeadline;
      var last = attempts > 0 || !inWindow;
      var result = readWithStrategies(path, readTextRaw, !last);
      if (result) {
        noteRead(path, result);
        return Promise.resolve(result.value);
      }
      attempts += 1;
      if (last) return Promise.resolve(null);
      return new Promise(function (resolve) {
        setTimeout(function () { resolve(step()); }, PACK_READ_RETRY_DELAY_MS);
      });
    }
    return step();
  }

  // ---------------------------------------------------------------- 事件系统

  function createEventTarget(target) {
    var listeners = {};
    target.addEventListener = function (type, handler) {
      if (typeof handler !== 'function') return;
      (listeners[type] = listeners[type] || []).push(handler);
    };
    target.removeEventListener = function (type, handler) {
      var list = listeners[type];
      if (!list) return;
      var index = list.indexOf(handler);
      if (index >= 0) list.splice(index, 1);
    };
    target.dispatchEvent = function (event) {
      var list = listeners[event && event.type] || [];
      for (var i = 0; i < list.length; i++) {
        try { list[i](event); } catch (e) { warn('事件处理失败 ' + (event && event.type) + ': ' + e); }
      }
    };
    target.__listenerCount = function (type) { return (listeners[type] || []).length; };
    return target;
  }

  function makeEvent(type, fields) {
    var event = { type: type, preventDefault: function () {}, stopPropagation: function () {} };
    for (var key in fields) if (Object.prototype.hasOwnProperty.call(fields, key)) event[key] = fields[key];
    return event;
  }

  // ---------------------------------------------------------------- canvas / 文档

  // 主画布来源：微信小游戏把"上屏画布"同时挂在全局上（`GameGlobal.canvas` / `canvas`），而
  // `wx.createCanvas()` 只有**首次**调用返回上屏画布，之后一律返回离屏画布。若引导代码先调过
  // 一次（引擎、第三方库、平台自身），我们拿到的就是离屏画布——画得再卖力也看不见（实测现象：
  // 控制台全绿、流程正常、画面全黑）。所以优先用平台已经给出的主画布，拿不到才去创建。
  var platformCanvas = (g.GameGlobal && g.GameGlobal.canvas) || g.canvas || null;
  var createdCanvas = null;
  var canvasSource = '';
  var canvas = (platformCanvas && typeof platformCanvas.getContext === 'function') ? platformCanvas : null;
  if (canvas) {
    canvasSource = 'platform-global';
  } else {
    createdCanvas = safeCall(wx, 'createCanvas');
    canvas = createdCanvas;
    canvasSource = canvas ? 'wx.createCanvas' : '';
  }
  if (!canvas || typeof canvas.getContext !== 'function') {
    throw new Error('wechat-adapter: 拿不到可用的 2D 主画布（global canvas / wx.createCanvas 都不可用）');
  }
  // 上屏画布是否与创建出来的是同一个对象：不是的话说明我们拿到的是离屏画布（必须换掉）。
  var canvasIsMain = !!(platformCanvas && canvas === platformCanvas);
  // 是否与 wx.createCanvas() 返回的是同一个对象（true 时说明"首次调用"语义被我们用对了）。
  var canvasIsCreated = !!(createdCanvas && canvas === createdCanvas);

  // **B8：原子模式开关已退休。** 这里原本是一个"原子模式"开关（默认开、可显式关回旧形态），
  // 它一次决定四件事：主画布归 Rust 宿主、2d 改道离屏、走 scene 档（`sim:'script'`）、帧源归宿主。
  // 旧世界删除后**没有可回退的旧形态**了，所以这四件事不再需要开关 —— 它们就是唯一形态。
  // 原先读它的两个入口（全局 `__RUSTLUA_RUST_SCENE_HOST` 与存储 `rustlua_rust_scene_host`）一并删除。

  // **B8：可见接管开关（`__RUSTLUA_ENGINE_HOST_VISIBLE`）已随原子模式退休。** 它的唯一作用曾是
  // "原子模式被显式关掉时仍跑接管形态"；原子模式现在是唯一形态，所以它没有可作用的场合。
  // 接管形态的**唯一**判据：原子模式（默认）或被显式要求的旧 VISIBLE 开关。
  // **B8：恒真** —— 原子宿主是唯一形态 ⇒ 可见接管恒开（原判据还看上面那个已删的开关）。
  var engineVisibleTakeover = true;

  // 2d 改道的读数（diag 用）。状态机：
  //   off            —— 已不存在：原子模式恒开（2d 不再有"照旧绑主画布"的形态）。
  //                    （本笔之前的旧生产形态，现在只由"显式关掉原子模式"得到）；
  //   redirected     —— 2d 已改道到离屏画布，主画布留给引擎宿主的 webgl2；
  //   fallback-main —— 改道画布拿不到 ⇒ 退回"2d 绑主画布"（画面优先于接管，host 会因此拿不到
  //                     webgl2，具名失败在 engine_host.reason 里，这是**可见**的降级不是静默）。
  var twoDRedirect = { status: 'off', source: '', note: '' };
  var redirectCanvas = null;

  // ── 微信侧 GL 能力读数（WP8 的设备验证仪器）────────────────────────────────
  // 为什么要有它：引擎的真渲染器是 wgpu/WebGL2（`gpu_host.rs`：`Backends::GL` +
  // `downlevel_webgl2_defaults()`），而"微信小游戏给不给 WebGL2"直接决定 WP8 在微信上能不能
  // 沿用同一条渲染通道。这个问题**只能在真机/开发者工具里**回答，所以把它做成随启动诊断回传的
  // 读数（进 `diagSnapshot().gl`），而不是留在文档里的猜测。
  //
  // 两条硬约束：
  //   ① **绝不用主画布试 GL**：一张画布只能有一个上下文（WP5 实测），在主画布上
  //      `getContext('webgl2')` 会把已经拿到的 2d 上下文毁掉，画面直接黑。
  //   ② 只用宿主声明过的入口：`wx.createOffscreenCanvas({type:'webgl'})` 优先（微信官方给的离屏
  //      GL 画布入口），拿不到才退回"再调一次 `wx.createCanvas()`"（首次之外的调用返回离屏画布）；
  //      两者都拿不到就如实留 `note`，不猜。
  // 两种 GL 版本各用**各自的**探针画布：失败的类型在实现里可能把画布锁给第一个请求，共用一张会让
  // "webgl2 不支持"顺带污染"webgl 也不支持"的结论。
  var glCapability = null;

  function glLimits(gl) {
    var out = { version: '', max_texture: 0, max_renderbuffer: 0, renderer: '' };
    try {
      out.version = String(gl.getParameter(gl.VERSION) || '');
      out.max_texture = Number(gl.getParameter(gl.MAX_TEXTURE_SIZE)) || 0;
      out.max_renderbuffer = Number(gl.getParameter(gl.MAX_RENDERBUFFER_SIZE)) || 0;
      var dbg = gl.getExtension ? gl.getExtension('WEBGL_debug_renderer_info') : null;
      if (dbg && dbg.UNMASKED_RENDERER_WEBGL) {
        out.renderer = String(gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) || '');
      }
    } catch (error) { /* 读数拿不全不影响启动 */ }
    return out;
  }

  function glProbeCanvas(info) {
    if (typeof wx.createOffscreenCanvas === 'function') {
      try {
        var off = wx.createOffscreenCanvas({ type: 'webgl', width: 1, height: 1 });
        if (off && typeof off.getContext === 'function') {
          info.source = 'wx.createOffscreenCanvas';
          return off;
        }
      } catch (error) { info.note = info.note || 'createOffscreenCanvas-threw'; }
    }
    var again = null;
    try { again = safeCall(wx, 'createCanvas'); } catch (error) { again = null; }
    if (!again || typeof again.getContext !== 'function') {
      info.note = info.note || 'no-probe-canvas';
      return null;
    }
    if (again === canvas) {
      info.main_canvas_skipped = true;
      info.note = info.note || 'probe-canvas-is-main';
      return null;
    }
    info.source = info.source || 'wx.createCanvas(offscreen)';
    return again;
  }

  // ── 引擎侧画布能力探针（WP8 第二片：把 WP7-50 的 wasm 导出接到真实调用点）────────────
  // 与上面 `glCapability` 的分工必须说清楚，否则两个读数会被混着读：
  //   * `glCapability` 问的是"**宿主**给不给 WebGL2"（JS 层：`canvas.getContext('webgl2')`）；
  //   * 这里问的是"**引擎自己**能不能在这张画布上起渲染栈"——走 wasm 里的 `probe_canvas_host`
  //     （wgpu instance → surface → adapter → 能力），也就是 WP8 真正要用的那条通道。
  // 两者结论不一致是有意义的：宿主给 WebGL2 而引擎起不来 = wgpu/后端的问题；反之 = 宿主限制。
  //
  // 导出只存在于带 `gpu2d` 特性的内核（`package.ps1 -Gpu2d`）。默认档没有 ⇒ 状态如实写
  // `unavailable`，**不是失败**："没编进去"与"编进去但宿主机不给"必须能分开读。
  //
  // 开关默认**关**：这条探针会真的去建 wgpu 实例/表面/适配器，属于要显式要求的动作。
  // 打开方式（二选一）：`globalThis.__RUSTLUA_GPU_PROBE === true`，或
  // `wx.setStorageSync('rustlua_gpu_probe', 1)`（真机上现场排查用后者）。
  var engineProbe = { status: 'off', reason: '', reports: [], source: '' };
  var engineProbePromise = null;
  var engineHostRef = null;
  // `attachWasmBridge` 里定义的微信同步读取器（`/project/…` → `pack/…`）。scene 档要在
  // begin **之前**用它预装 vfs reader provider（见 engineHostStart 里的注释），所以提到模块级。
  var wechatReadFileRef = null;

  function engineProbeEnabled() {
    try { if (globalThis.__RUSTLUA_GPU_PROBE === true) return true; } catch (error) { /* 无全局对象 */ }
    try {
      if (typeof wx.getStorageSync === 'function') {
        var stored = wx.getStorageSync('rustlua_gpu_probe');
        if (stored === true || stored === 1 || stored === '1') return true;
      }
    } catch (error) { /* 存储不可用不影响结论 */ }
    return false;
  }

  // 专用 GPU 画布（探针与引擎宿主共用）。**每次调用都新开一张**，理由两条：
  //   ① 一张画布只能有一个上下文——绝不能用主画布（2d），否则画面直接黑；
  //   ② 两次探测/宿主之间也不能共用：JS 层探针可能已经在旧画布上拿过 webgl2 上下文，
  //      wgpu 再取会拿到同一个上下文，把两边结论耦在一起（一个失败会污染另一个）。
  function dedicatedGpuCanvas(info, width, height) {
    if (typeof wx.createOffscreenCanvas === 'function') {
      try {
        var off = wx.createOffscreenCanvas({ type: 'webgl', width: width, height: height });
        if (off && typeof off.getContext === 'function') { info.source = 'wx.createOffscreenCanvas'; return off; }
      } catch (error) { info.note = info.note || 'createOffscreenCanvas-threw'; }
    }
    var again = null;
    try { again = safeCall(wx, 'createCanvas'); } catch (error) { again = null; }
    if (!again || typeof again.getContext !== 'function' || again === canvas) {
      info.note = info.note || (again === canvas ? 'probe-canvas-is-main' : 'no-probe-canvas');
      return null;
    }
    info.source = 'wx.createCanvas(offscreen)';
    return again;
  }

  function engineProbeCanvas(info) {
    return dedicatedGpuCanvas(info, 16, 16);
  }

  function probeEngineCanvas() {
    if (engineProbePromise) return engineProbePromise;
    // **所有出口都必须写完读数**：状态写进 `data-rustlua-engine-probe`、理由进 diag、并打一行日志。
    // 第一版把 `off`/`unavailable` 做成了"提前 return 什么都不写"——结果验证侧读到的是
    // `engine_probe=none`，与"这段代码根本没接线"完全同形。诊断读数不许有这种哑口。
    var started = Date.now();
    var finish = function () {
      try { bodyElement.setAttribute('data-rustlua-engine-probe', engineProbe.status); }
      catch (error) { /* 属性写不进去不影响结论 */ }
      log('wechat-adapter: 引擎画布探针 status=' + engineProbe.status + ' source=' + (engineProbe.source || 'none') +
        ' ms=' + (Date.now() - started) + ' reason=' + (engineProbe.reason || ''));
      return engineProbe;
    };
    if (!engineProbeEnabled()) {
      engineProbe.status = 'off';
      engineProbe.reason = 'switch-off';
      engineProbePromise = Promise.resolve(finish());
      return engineProbePromise;
    }
    var host = engineHostRef;
    var exportFn = (host && typeof host.probe_canvas_host === 'function') ? host.probe_canvas_host : null;
    if (!exportFn) {
      // 两种"没有导出"必须分开读：内核没编 GPU 路径（unavailable）vs 内核还没加载完（not-loaded，
      // 说明调用时机错了——第一版就是在这里踩的：同步调用时 `engineHostRef` 还是 null）。
      engineProbe.status = host ? 'unavailable' : 'failed';
      engineProbe.reason = host ? 'kernel-without-gpu2d' : 'kernel-not-loaded-yet';
      engineProbePromise = Promise.resolve(finish());
      return engineProbePromise;
    }
    var info = { source: '', note: '' };
    var target = engineProbeCanvas(info);
    engineProbe.source = info.source;
    if (!target) {
      engineProbe.status = 'failed';
      engineProbe.reason = 'no-probe-canvas:' + (info.note || 'unknown');
      engineProbePromise = Promise.resolve(finish());
      return engineProbePromise;
    }
    var collect = function (line) { engineProbe.reports.push(String(line).slice(0, 300)); };
    engineProbePromise = Promise.resolve()
      .then(function () { return exportFn(target, collect); })
      .then(function (summary) {
        engineProbe.status = 'ok';
        engineProbe.reason = String(summary || '').slice(0, 300);
        return engineProbe;
      })
      .catch(function (error) {
        // 失败原因**原样**带出来（wasm 侧是 `probe: …` 具名原因，见 `gpu_host.rs`）：
        // 现场只靠这一行区分"宿主机不给 WebGL2"与"我们的路径有问题"。
        engineProbe.status = 'failed';
        engineProbe.reason = String((error && error.message) || error).slice(0, 300);
        return engineProbe;
      })
      .then(finish);
    return engineProbePromise;
  }

  // 给验证侧一个可 await 的入口（harness 在打完自报前会等它落位）。真机不需要它。
  try { globalThis.__RUSTLUA_ENGINE_PROBE = probeEngineCanvas; } catch (error) { /* 无全局对象 */ }

  // ── 引擎宿主（WP8 主线·第二片）：真的起一个宿主，并按帧驱动它 ────────────────────────
  // 与探针的分工：探针只回答"能不能起渲染栈"；这里**起一个宿主**并按帧 `engine_host_frame`，
  // 于是微信侧第一次有"引擎自己在画布上出帧"的能力（清屏 + 呈现，状态按帧回报）。
  //
  // （B8：原子模式已是唯一形态，下面那段旧开关说明只作沿革保留。）
  // （或 `wx.setStorageSync('rustlua_engine_host', 1)`）在原子模式被显式关掉时仍可用——
  // 它是"只起宿主、不改道画面"的那条旧通路，harness 既有跑法用它。
  //
  // 输入走**同一份** `RLFI` 批量，且**只有一个所有者**：运行时（web-runtime.js）把设备快照打成
  // `RLFI` 发给 `lua_input_frame`，适配层那层包装只**计数**再原样转给同一个 wasm 导出
  // （`result.host.lua_input_frame(bytes)`——不发明第二套协议、不搬字节）。宿主自己那条帧路
  // （`engineHostDriveFrame`）**恒传 `undefined`**，所以定长 ABI 里的 `input_applied` 恒为 0：
  // 这就是"宿主不是第二个注入者"的**可读证据**，而 `runtime_input_frames` 才是注入计数。
  // 为什么原子模式下也不把注入搬给宿主：`apply()` 的返回值是 wasm 侧真的改了几位，进
  // `input frames=N bits=M` 读数，是"两边对'这一帧该变哪些位'理解不一致"的第一手线索（恒 0 =
  // 异常信号）。搬走等于让宿主在**下一帧**才解码这一批 ⇒ 这一帧拿不到真值 ⇒ 那个守卫读数变成
  // 恒 0 的假信号。搬家的前提是删掉旧 JS 世界那一笔（那时这个读数也一起没了），不是本笔。
  var engineHost = {
    status: 'off', reason: '', source: '', begin: '', frames: 0, steps: 0,
    presented: 0, failed: 0, clamped: 0, inputApplied: 0, lastState: null,
    // 本次宿主是不是接管的**主画布**（方案 A）；diag 与 harness 都读它，不猜开关组合。
    visibleTakeover: false,
    // 帧源（谁拥有帧泵）：`wx.requestAnimationFrame`（原子模式且有它）或 `setTimeout`（兜底）。
    // 只留字符串读数——"帧率不对 / 泵没起来"这类现场，第一个要问的就是它挂在哪个帧源上。
    frameSource: '',
    // 收到窗口尺寸变化并**成功通知宿主**的次数（engine_host_resize）。为什么要有这个读数：
    // 真机横竖屏切换后「画面比例不对」的第一个问题就是「宿主到底知不知道尺寸变了」——
    // 只改 canvas 属性也能让 presented= 照常增长，从读数上看不出区别。
    resizeCalls: 0,
    // B4：**宿主自己的**输入生产者跑了多少帧 / 落不进线格式的键名个数 / 最近一次 flush 的读数。
    // 为什么必须有这两个数：输入所有权从旧运行时交到宿主之后，"旧通道还用不用"（
    // `runtimeInputFrames`）与"新通道有没有在工作"必须能分开读——只盯前者会把
    // "两边都没动"读成"交接完成"。
    hostInputFrames: 0, hostInputDroppedKeys: 0, hostInputLast: "",
    // 多指（微信侧）：到达每指导出的触摸事件数 / "同时按住多根"的帧数 / 当前按住的 id 集合。
    hostTouchEvents: 0, hostTouchMulti: 0, hostTouchActive: {},
    // ④ 前后台门控计数：hidden 期间被跳过、没有送去 `engine_host_frame` 的帧数。
    hiddenSkips: 0,
    // ⑥ begin summary 里的 Rust 侧线性内存（`wasm_mem_bytes`），从 JSON 里抠出来单记——
    // `begin` 字段只留 400 字符，整包塞进 diag 会被截尾。
    memBegin: null,
    // 定长 ABI（WP7-55）：每帧搬一个 Float64Array，不再每帧 JSON.parse。
    abi: '', abiVersion: 0, slot: null, out: null,
    // 宿主是否负责跑 Lua 固定步；以及"老 JS 世界也在跑"的冲突读数。
    tick: false, tickConflict: false, simSteps: 0, tickFailures: 0, runtimeFixedFrames: 0,
    // `runtimeRunSteps` 才是"运行时**真的**跑了固定步"的证据：`lua_fixed_frame` 只是每帧问步数
    // （宿主接管时也照样问），拿它当冲突证据会把"正常"误判成"双步"——WP7-57 修的就是这个。
    runtimeRunSteps: 0,
    // 运行时这一局注入过多少次输入批量（= 帧数）：WP7-58 用它钉"输入只有一个所有者"。
    runtimeInputFrames: 0
  };
  // WP7-58：宿主不再接收输入批量（注入归运行时），所以这个"留给宿主的那一份"彻底删掉——
  // 留着它就是留着第二个所有者。
  var engineHostPromise = null;
  var engineHostLastMs = 0;
  var engineHostMinMs = 0;

  function engineHostEnabled() {
    // 原子模式（默认开）：主画布、2d 改道、scene 档、帧源都归宿主——它就是"宿主开着"。
    return true; // B8：原子宿主是唯一形态（下面几条旧判据已不可达，待单独一轮清扫）
    // 可见接管开关**蕴含**引擎宿主开关：接管形态下宿主是画面所有者，不存在"开接管不开宿主"。
    if (engineVisibleTakeover) return true;
    try { if (globalThis.__RUSTLUA_ENGINE_HOST === true) return true; } catch (error) { /* 无全局对象 */ }
    try {
      if (typeof wx.getStorageSync === 'function') {
        var stored = wx.getStorageSync('rustlua_engine_host');
        if (stored === true || stored === 1 || stored === '1') return true;
      }
    } catch (error) { /* 存储不可用不影响结论 */ }
    return false;
  }

  // 宿主的**第二个**开关：谁跑 Lua 的固定步。默认关，因为老 JS 世界每帧也在跑
  // （`lua_fixed_frame` + 自己的 update 循环）；两边同时跑就是把世界推进两次。
  // 打开它 = "引擎宿主拥有整个世界"那条路，接线到生产路径时要连老世界的 tick 一起关。
  // **原子模式下它被忽略**（恒 `tick:'none'`）：原子模式走 scene 档，而 scene 与 `tick:'lua'`
  // 在 begin 里互斥（具名拒绝）——scene 档的固定步由共享宿主 `EngineCore::frame` 自己推进。
  function engineHostTickEnabled() {
    try { if (globalThis.__RUSTLUA_ENGINE_HOST_TICK === true) return true; } catch (error) { /* 无全局对象 */ }
    try {
      if (typeof wx.getStorageSync === 'function') {
        var stored = wx.getStorageSync('rustlua_engine_host_tick');
        if (stored === true || stored === 1 || stored === '1') return true;
      }
    } catch (error) { /* 存储不可用不影响结论 */ }
    return false;
  }

  // 像素回读的**显式**开关（#19 归因判别用）：默认跟随接管形态（验收要像素证据），
  // 给 true/false（或存储值 `0`/`off`）才偏离默认。#19 用它分流跑过判别，实测结论已落账：
  // 事后 `toDataURL` 在接管形态下**不可靠**——同一形态两次冷启动一次给整幅 RGBA(0,0,0,0)、
  // 一次给到活画面（回读落在合成清空之前/之后的竞态），所以画面归属的证据只认引擎侧
  // present 后同任务的 readPixels（见 host_runtime.rs `read_pixel_sample` 的口径注释）。
  // 未置任何值时返回 `null` = 沿用默认，生产与既有验收读数逐字节不变。
  function engineHostPixelSampleOverride() {
    try {
      var value = globalThis.__RUSTLUA_ENGINE_HOST_PIXEL_SAMPLE;
      if (value === true || value === 1 || value === '1') return true;
      if (value === false || value === 0 || value === '0' || value === 'off') return false;
    } catch (error) { /* 无全局对象 */ }
    try {
      if (typeof wx.getStorageSync === 'function') {
        var stored = wx.getStorageSync('rustlua_engine_host_pixel_sample');
        if (stored === true || stored === 1 || stored === '1') return true;
        if (stored === false || stored === 0 || stored === '0' || stored === 'off') return false;
      }
    } catch (error) { /* 存储不可用不影响结论 */ }
    return null;
  }

  // 字符串形态的宿主开关（scene 档路径/路由名用）：全局优先、存储兜底，空串 = 没开。
  // 与其它开关同款读取顺序，只是值类型是字符串（布尔开关的 `=== true` 判定在这里不适用）。
  function engineHostSwitchString(globalName, storageKey) {
    try {
      var value = globalThis[globalName];
      if (typeof value === 'string' && value.length > 0) return value;
    } catch (error) { /* 无全局对象 */ }
    try {
      if (typeof wx.getStorageSync === 'function') {
        var stored = wx.getStorageSync(storageKey);
        if (typeof stored === 'string' && stored.length > 0) return stored;
      }
    } catch (error) { /* 存储不可用不影响结论 */ }
    return '';
  }

  // 「谁在推进世界」的**唯一**标记：运行时（`web-runtime.js`）每帧读它。
  // 宿主起不来/降级时必须由这里清掉 —— 否则"宿主死了"会表现成"世界冻住"（脚本还在画，
  // 但 update 再也不跑），而这类"画面正常、逻辑不动"的故障最难查。
  function engineHostOwnsTick(on) {
    try { globalThis.__RUSTLUA_ENGINE_HOST_OWNS_TICK = !!on; } catch (error) { /* 无全局对象 */ }
    if (!on) {
      try { globalThis.__RUSTLUA_ENGINE_HOST_STEPS = 0; } catch (error) { /* 无全局对象 */ }
    }
  }

  // **启动状态的归属交接**（B5）：`ok` 以前由 JS 世界（web-runtime.js 的 markBoot）写，而 JS 世界
  // 删除后它就没写者了 —— 诊断与回归门读的正是它。适配层是**会留下**的那一层，所以由它补。
  // **只补不盖**：启动失败时本层自己写过 failed（见下面 setAttribute('data-rustlua-boot','failed')
  // 那处），绝不能在这里被"那边一切正常"盖掉。与 Rust 孪生 `boot_state_to_publish` 成对改。
  function publishBootOk() {
    try {
      var current = bodyElement.getAttribute ? bodyElement.getAttribute('data-rustlua-boot') : null;
      if (current === 'failed' || current === 'ok') return;
      bodyElement.setAttribute('data-rustlua-boot', 'ok');
    } catch (error) { /* 属性写不进去不影响结论 */ }
  }

  function engineHostFinish() {
    try { bodyElement.setAttribute('data-rustlua-engine-host', engineHost.status); }
    catch (error) { /* 属性写不进去不影响结论 */ }
    if (engineHost.status === 'running') publishBootOk();
    log('wechat-adapter: 引擎宿主 status=' + engineHost.status + ' source=' + (engineHost.source || 'none') +
      ' frames=' + engineHost.frames + ' steps=' + engineHost.steps + ' presented=' + engineHost.presented +
      (engineHost.reason ? ' reason=' + engineHost.reason : ''));
    return engineHost;
  }

  function engineHostStart() {
    if (engineHostPromise) return engineHostPromise;
    if (!engineHostEnabled()) {
      engineHost.status = 'off';
      engineHost.reason = 'switch-off';
      engineHostPromise = Promise.resolve(engineHostFinish());
      return engineHostPromise;
    }
    var host = engineHostRef;
    var exportFn = (host && typeof host.engine_host_begin === 'function') ? host.engine_host_begin : null;
    if (!exportFn) {
      engineHost.status = host ? 'unavailable' : 'failed';
      engineHost.reason = host ? 'kernel-without-gpu2d' : 'kernel-not-loaded-yet';
      engineHostPromise = Promise.resolve(engineHostFinish());
      return engineHostPromise;
    }
    var info = { source: '', note: '' };
    // 宿主画布按**上屏尺寸**开（它要负责整屏呈现），而不是探针那种 16×16 的连通性画布。
    var width = Math.max(1, Math.round(screenWidth * (pixelRatio || 1)));
    var height = Math.max(1, Math.round(screenHeight * (pixelRatio || 1)));
    var target;
    if (engineVisibleTakeover) {
      // 接管形态：宿主直接绑**主画布**。此刻主画布还是干净的——2d 从没在它上面绑过
      // （getContext 包装在第一次 '2d' 请求时就把它改道到离屏，见 ensureTwoDRedirectCanvas），
      // webgl2 因此能开出来。能开 webgl2 这件事本身就是这片验收要回答的问题（模拟器/真机都未证）。
      target = canvas;
      info.source = 'main-canvas(visible-takeover)';
      // 尺寸以主画布现状为准（平台给的就是上屏像素尺寸），拿不到才退回屏幕计算值。
      width = Math.max(1, canvas.width || width);
      height = Math.max(1, canvas.height || height);
    } else {
      target = dedicatedGpuCanvas(info, width, height);
    }
    engineHost.source = info.source;
    engineHost.visibleTakeover = engineVisibleTakeover;
    if (!target) {
      engineHost.status = 'failed';
      engineHost.reason = 'no-host-canvas:' + (info.note || 'unknown');
      engineHostPromise = Promise.resolve(engineHostFinish());
      return engineHostPromise;
    }
    // 固定步归属：`tick:'lua'` 与 scene 档在 begin 里**互斥**（具名拒绝，host_runtime.rs 的
    // `scene_spec.is_some() && tick_lua` 那条），而原子模式走的就是 scene 档、固定步由共享宿主的
    // `frame()` 推进 ⇒ 原子模式恒 `none`。旧开关 `__RUSTLUA_ENGINE_HOST_TICK` 只在原子模式被
    // 显式关掉时才有意义（那条路上没有 scene，也就没有互斥）。
    var tickRequested = engineHostTickEnabled();
    var tickWanted = false; // B8：原子模式下固定步归宿主；原判据 tickRequested && !rustSceneHost 恒假
    if (tickRequested) {
      warn('wechat-adapter: 原子模式下固定步归宿主（scene 档由共享核心帧链推进），忽略 '
        + '__RUSTLUA_ENGINE_HOST_TICK —— tick:"lua" 与 scene 互斥');
    }
    // scene 档路径（P1 共享核心；harness 的 scene 专跑用它）：显式给一个包内相对 .scene 路径，
    // 宿主就走与浏览器**同一条**渲染链（Renderer + RustScene 真场景），而不是 clear-color 形态。
    var scenePath = engineHostSwitchString('__RUSTLUA_ENGINE_HOST_SCENE', 'rustlua_engine_host_scene');
    // 是不是 scene 档。**不能用 `scenePath` 当判据**：原子模式没有场景路径，而下面的 vfs 预装与
    // 帧泵都只在 scene 档成立——判据写成 `scenePath &&` 会让原子模式**泵根本不启动**（presented
    // 恒 0，而其余读数全绿），这是本笔最容易漏掉的一处。
    var sceneMode = true; // B8：原子宿主恒开 ⇒ scene 档恒成立（原判据还看 scenePath）
    var optionsObj = {
      updates_per_second: 60,
      max_catch_up_steps: 8,
      max_frame_seconds: 0.25,
      canvas_width: width,
      canvas_height: height,
      // `tick: 'lua'` = 由宿主推进 Lua 固定步。老 JS 世界每帧也在跑，两边同时跑 =
      // 世界推进两次 ⇒ 只有显式打开第二个开关、**且不在原子模式下**才走这条路
      // （原子模式恒 `none`，见上面 `tickWanted` 的算式）。
      tick: tickWanted ? 'lua' : 'none',
      // 接管形态固定开像素回读：验收要"主画布真的被宿主画出来了"的**像素证据**，
      // 而不是只有 status=running（口径见 host_runtime.rs 的 preserveDrawingBuffer 注释——
      // 唯一可信窗口是 present 后同任务的引擎侧 readPixels，JS 事后 toDataURL 只会截到空白）。
      // `__RUSTLUA_ENGINE_HOST_PIXEL_SAMPLE` 显式给值时偏离该默认（#19 归因判别，见上）。
      pixel_sample: (function () {
        var override = engineHostPixelSampleOverride();
        return override === null ? engineVisibleTakeover : override;
      })()
    };
    if (scenePath) {
      // 显式给了场景路径 ⇒ 照旧同时给 `scene` 与 `sim`（`sim` 缺省即 `script`）——harness 的
      // scene 专跑走这条，逐字不变。
      optionsObj.scene = scenePath;
      var simRoute = engineHostSwitchString('__RUSTLUA_ENGINE_HOST_SIM', 'rustlua_engine_host_sim');
      if (simRoute) optionsObj.sim = simRoute;
      log('wechat-adapter: 引擎宿主将以 scene 档启动 scene=' + scenePath + (simRoute ? ' sim=' + simRoute : ''));
    } else {
      // 原子模式：只给 `sim`、**不写 `scene` 键**。`sim:'script'`（`SimRoute::EngineStartup`）让
      // **引擎自己解析启动场景**，这条路的 `scene_path` 只进读数 ⇒ 宿主可以不带场景路径直接进
      // scene 档（host_runtime.rs::option_scene 的 `(None, Some(name))` 分支；另外三条路线仍必须
      // 同时给 `scene`，它们由宿主提供场景字节）。原子模式只认这一个形态，所以这里不再透传
      // `__RUSTLUA_ENGINE_HOST_SIM`——"一个原子模式"的意思就是没有第二种组合。
      optionsObj.sim = 'script';
      log('wechat-adapter: 引擎宿主以原子模式启动 sim=script（启动场景由引擎自己解析，不写 options.scene）');
    }
    var options = JSON.stringify(optionsObj);
    // scene 档（**原子模式与显式 scene 路径都要**，所以判据是 `sceneMode` 而不是 `scenePath`）
    // **预装 reader provider**（实测根因）：begin 里 `RustScene::boot` 会调
    // `install_resource_provider`（= 预加载表 provider，微信侧表恒空），vfs 槽是一次性的——
    // 它先占了槽，运行时随后 `lua_boot` 装 reader 时"沿用现有"，main.lua 必炸
    // `vfs: resource not found: /project/scripts/main.lua` ⇒ boot=failed、rAF 链死。
    // 先用真 reader 装槽：RustScene 的读取落到真字节，运行时再 boot 沿用同一 provider。
    if (sceneMode && typeof host.lua_boot_with_reader === 'function' && wechatReadFileRef) {
      try {
        host.lua_boot_with_reader(wechatReadFileRef);
        log('wechat-adapter: scene 档已预装 reader provider（vfs 槽归适配层读取器；mode='
          + (scenePath ? 'scene-path=' + scenePath : 'atomic sim=script') + '）');
      } catch (error) {
        warn('wechat-adapter: scene 档预装 reader provider 失败 -> ' + hostError(error));
      }
    }
    if (tickWanted) {
      log('wechat-adapter: 引擎宿主将接管固定步（tick=lua）——运行时会因此停掉自己的固定步；'
        + '若宿主起不来，这个标记会被清掉（世界不会冻住）');
    }
    // 先按"没接管"起：只有 begin **成功**之后才把这个标记交给宿主。中途任何失败路径
    // （画布、设备、管线）都不需要各自记着清标记——它们本来就没设置过。
    engineHostOwnsTick(false);
    var report = function (line) { log('wechat-adapter: 引擎宿主 ' + String(line).slice(0, 300)); };
    engineHostPromise = Promise.resolve()
      .then(function () { return exportFn(target, options, report); })
      .then(function (summary) {
        engineHost.status = 'running';
        engineHost.begin = String(summary || '').slice(0, 400);
        // ⑥ Rust 侧 begin 基线：从 summary JSON 里抠 `wasm_mem_bytes`（口径见 host_runtime.rs）。
        var memMatch = /"wasm_mem_bytes":(\d+)/.exec(String(summary || ''));
        engineHost.memBegin = memMatch ? Number(memMatch[1]) : null;
        engineHost.tick = tickWanted;
        // 到这里宿主**真的起来了**（设备、管线、布局都拿到了）：如果它是来接固定步的，
        // 现在才把"世界由它推进"的标记交给它。放在 `.catch` **之前**，所以后面任何失败都会走
        // 下面的降级路径清掉标记。
        if (tickWanted) engineHostOwnsTick(true);
        engineHostLastMs = Date.now();
        engineHostMinMs = engineHostLastMs;
        // 定长 ABI 的布局由 wasm 给（`engine_host_frame_layout_json`）：偏移只有这一份来源，
        // 适配层不再自己维护字段表——两边各写一份就是"字段错位但都看着正常"的老路。
        return engineHostFrameAbi();
      })
      .then(function () {
        // 帧 ABI 到位后才有"能出帧"的前提；scene 档从这里自带泵，其余形态照旧走 afterFrame。
        // 判据是 `sceneMode`：原子模式**没有** `scenePath`，写成 `scenePath &&` 会让泵不启动。
        if (sceneMode && engineHost.status === 'running') startEngineHostScenePump();
      })
      .catch(function (error) {
        engineHost.status = 'failed';
        engineHost.reason = String((error && error.message) || error).slice(0, 300);
        // 宿主没了 ⇒ 标记必须还回去，否则世界会冻在"脚本还在画、update 不再跑"的状态。
        engineHostOwnsTick(false);
        return engineHost;
      })
      .then(engineHostFinish);
    return engineHostPromise;
  }

  // 取一次布局并建"槽位名 → 下标"的表；拿不到就具名失败（不退回 JSON：同一份产物里
  // 两个入口同时存在，退回等于把"版本不匹配"藏起来）。
  function engineHostFrameAbi() {
    var host = engineHostRef;
    if (!host || typeof host.engine_host_frame_layout_json !== 'function' ||
      typeof host.engine_host_frame_into !== 'function') {
      engineHost.status = 'failed';
      engineHost.reason = 'host: frame-abi-missing（wasm 侧没有定长帧 ABI：engine_host_frame_layout_json / engine_host_frame_into）';
      return engineHost;
    }
    var layout;
    try {
      layout = JSON.parse(host.engine_host_frame_layout_json());
    } catch (error) {
      engineHost.status = 'failed';
      engineHost.reason = 'host: frame-abi-json: ' + String((error && error.message) || error).slice(0, 200);
      return engineHost;
    }
    var slot = {};
    for (var i = 0; i < layout.slots.length; i++) slot[layout.slots[i]] = i;
    if (!(layout.len > 0) || layout.slots.length !== layout.len) {
      engineHost.status = 'failed';
      engineHost.reason = 'host: frame-abi-shape: len=' + layout.len + ' slots=' + layout.slots.length;
      return engineHost;
    }
    engineHost.slot = slot;
    engineHost.out = new Float64Array(layout.len);
    engineHost.abi = String(layout.abi || '');
    engineHost.abiVersion = Number(layout.version) || 0;
    return engineHost;
  }

  // scene 档的**自泵**标记。为什么 scene 档必须自带帧泵：`afterFrame` 挂在运行时（web-runtime.js）
  // 的 rAF 回调上，而 scene 档的语义是"引擎宿主自己起场景、跑自己的模拟"——它是画面与世界的
  // 所有者，出帧不该依赖被它接管的那一方（实测：scene 档 begin 成功、mode=scene、rust(boot ok)，
  // 但运行时 boot=failed、rAF 链死 ⇒ presented 恒 0）。
  //
  // 帧源（本笔）：`host_runtime.rs` 顶部契约写明帧源应由 `requestAnimationFrame` /
  // `wx.requestAnimationFrame` 决定 ⇒ **原子模式优先用 `wx.requestAnimationFrame`**（宿主就是
  // 帧所有者：`tick:'none'`、scene 档固定步由共享宿主的 `frame()` 推进），没有它才退回
  // `setTimeout(tick, 16)`。旧形态（只显式开了 VISIBLE 的 harness 通路）仍走 `setTimeout`：
  // 当初选它正是为了**不借**运行时的 rAF 队列（同一理由见下面 preBootUi 那两条刻意的约束），
  // 而那条路上宿主还不是帧所有者。
  // 实际挂在哪个帧源上写进 `engineHost.frameSource`——"谁拥有帧源"必须是可读读数，否则
  // "帧率不对 / 泵没起来"只能靠猜；读数记的是**实际**在用的那个，不是意图。
  var engineHostScenePump = false;

  function startEngineHostScenePump() {
    if (engineHostScenePump) return;
    engineHostScenePump = true;
    var useRaf = false;
    try { useRaf = !!(wx && typeof wx.requestAnimationFrame === 'function'); } catch (error) { useRaf = false; }
    engineHost.frameSource = useRaf ? 'wx.requestAnimationFrame' : 'setTimeout';
    log('wechat-adapter: 引擎宿主 scene 档自泵启动（帧驱动不再依赖运行时 rAF）frameSource='
      + engineHost.frameSource);
    var schedule = null;
    var tick = function () {
      if (engineHost.status !== 'running') { engineHostScenePump = false; return; }
      engineHostDriveFrame();
      schedule();
    };
    schedule = useRaf ? function () {
      try {
        wx.requestAnimationFrame(tick);
      } catch (error) {
        // 平台帧源中途不可用 ⇒ 落到定时器，并把读数改成**实际**在用的那个。
        engineHost.frameSource = 'setTimeout';
        setTimeout(tick, 16);
      }
    } : function () { setTimeout(tick, 16); };
    schedule();
  }

  // 每帧驱动宿主。**任何异常都必须降级成 failed 并停帧**：这是诊断通道，不许把游戏主循环掀翻。
  function engineHostDriveFrame() {
    if (engineHost.status !== 'running') return;
    var host = engineHostRef;
    if (!host || typeof host.engine_host_frame_into !== 'function' || !engineHost.slot) return;
    // ④ 与 gpu_host 帧循环同口径：hidden 期间不出帧。真机上 wx 的 rAF 本身会停，但"会停"
    // 是平台行为不是我们的保证（模拟器/异常宿主仍在给帧），所以这道门必须在。
    // 重开后的第一帧 dt 从当下起算（复位 engineHostLastMs），隐藏的时长不会积成一次巨大 dt。
    if (documentHidden) {
      engineHost.hiddenSkips += 1;
      engineHostLastMs = Date.now();
      return;
    }
    var now = Date.now();
    var dtMs = Math.max(0, now - engineHostLastMs);
    engineHostLastMs = now;
    // 输入批量**不转交**：注入归运行时（WP7-58），宿主只推进固定步。传 undefined 让宿主如实
    // 报"没注入过"——定长 ABI 里的 `input_applied` 恒为 0，这就是"宿主不是第二个注入者"的证据。
    // **原子模式（本笔默认形态）下这条一字不变**：设备输入仍只经运行时的 `lua_input_frame` 注入
    // （见门面那一段的复核），所以 `input_applied` 继续恒 0、`runtime_input_frames` 继续增长——
    // 两个读数一起读就是"一个输入所有者"的证据。下面那条 tick 冲突门只在 `engineHost.tick`
    // （= `tick:'lua'`）成立时才判；原子模式 `tick:'none'`，所以老世界照跑**不算**冲突。
    // 「一个世界只能有一个 tick」：宿主接管固定步之后，运行时**不许**再跑固定步。
    // 证据必须是"真的跑了"（`runtimeRunSteps`），不是"问了步数"——`lua_fixed_frame` 每帧都会问
    // （宿主接管时也照样问，才能拿到丢帧读数），拿它当证据会把正常态误判成双步（WP7-57 修的）。
    // 一旦真的同时跑：判 failed + 把标记还给运行时（宁可回到"老世界单跑"，也不要双步或冻住）。
    if (engineHost.tick && engineHost.runtimeRunSteps > 0) {
      engineHost.tickConflict = true;
      engineHost.status = 'failed';
      engineHost.reason = 'host: tick-conflict: 运行时与宿主同时推进世界（runtime_run_steps=' +
        engineHost.runtimeRunSteps + '）';
      engineHostOwnsTick(false);
      engineHostFinish();
      return;
    }
    try {
      // B4：先把这一帧攒下的事件取成一份线格式落到 `LATEST_FRAME`，**再**驱动帧。顺序反了，
      // 本帧的输入要等下一帧才被模拟看到（症状是"按键慢一帧"，而读数上看不出来）。
      // B4 激活：先把这一帧攒下的事件取成一份线格式落到 `LATEST_FRAME`，**再**驱动帧（顺序反了会慢一帧）。
      // 上游 `engineHostDriveFrame` 只在 `status==="running"` 时才被调，所以这里不必再判一次。
      // （离线宿主起不来 ⇒ 泵根本不跑 ⇒ 这一段进不去，所以"离线两条通道都没动"是环境的必然。）
      if (typeof host.engine_host_input_flush === 'function') {
        try {
          engineHost.hostInputLast = String(host.engine_host_input_flush()).slice(0, 200);
          engineHost.hostInputFrames += 1;
        } catch (error) {
          warn('wechat-adapter: engine_host_input_flush 失败 -> ' + hostError(error));
        }
      }
      var written = host.engine_host_frame_into(dtMs, undefined, engineHost.out);
      if (written !== engineHost.out.length) {
        throw new Error('宿主帧 ABI 长度不符：写入 ' + written + '，布局 ' + engineHost.out.length);
      }
      var at = engineHost.slot;
      var out = engineHost.out;
      engineHost.frames += 1;
      engineHost.steps += out[at.steps];
      engineHost.presented = out[at.presented_total];
      engineHost.failed = out[at.failed_presents];
      engineHost.inputApplied = out[at.input_applied];
      engineHost.simSteps = out[at.tick_steps_total];
      engineHost.tickFailures = out[at.tick_failures];
      if (out[at.clamped] === 1) engineHost.clamped += 1;
      // 宿主刚推进了几步：运行时的**帧级**阶段（相机抖动、`__rustlua_dt`、动画积分）要按它来，
      // 否则那些"每帧一次"的量会因为运行时自己的 steps=0 而停摆。
      if (engineHost.tick) {
        try { globalThis.__RUSTLUA_ENGINE_HOST_STEPS = out[at.steps]; } catch (error) { /* 无全局对象 */ }
      }
      // 只留一小撮字段供 diag 用（不再是整份 JSON）。
      engineHost.lastState = {
        frame_index: out[at.frame_index],
        steps: out[at.steps],
        delta_ms: out[at.delta_ms],
        accumulated_ms: out[at.accumulated_ms],
        dropped_total: out[at.dropped_total],
        tick_steps_total: out[at.tick_steps_total]
      };
    } catch (error) {
      engineHost.status = 'failed';
      engineHost.reason = String((error && error.message) || error).slice(0, 300);
      // 降级 ⇒ 把 tick 还给运行时（否则世界会冻住：脚本照画，update 不再跑）。
      engineHostOwnsTick(false);
      engineHostFinish();
    }
  }

  // 给验证侧一个可 await 的入口（harness 打完自报前会等它落位）。真机不需要它。
  // **入口名必须与开关名不同**：第一版这里复用了 `__RUSTLUA_ENGINE_HOST`（开关名），
  // 于是模块加载时这句赋值把 harness 置的 `true` 覆盖成了函数 ⇒ `=== true` 恒假 ⇒
  // 开关永远读成 `switch-off`。两个开关一起开的差分实验一眼劈开了这个现象：同一进程里
  // 探针 `unavailable`、宿主 `switch-off`。探针当初用两个名字（`__RUSTLUA_GPU_PROBE` 开关 /
  // `__RUSTLUA_ENGINE_PROBE` 入口）是对的，这里照抄那个命名纪律。
  try { globalThis.__RUSTLUA_ENGINE_HOST_READY = engineHostStart; } catch (error) { /* 无全局对象 */ }

  function probeGlCapability() {
    if (glCapability) return glCapability;
    var info = {
      probed: false, source: '', webgl2: false, webgl: false,
      main_canvas_skipped: false, note: '', limits: null
    };
    // 先落位：diagSnapshot 会被多处调用，探测只做一次（也顺手挡住递归）。
    glCapability = info;
    var tryContext = function (kind) {
      var target = glProbeCanvas(info);
      if (!target) return null;
      try { return target.getContext(kind) || null; } catch (error) { return null; }
    };
    var gl2 = tryContext('webgl2');
    if (gl2) {
      info.probed = true;
      info.webgl2 = true;
      info.limits = glLimits(gl2);
    } else {
      var gl1 = tryContext('webgl');
      if (gl1) {
        info.probed = true;
        info.webgl = true;
        info.limits = glLimits(gl1);
      }
    }
    return info;
  }

  // 画布跟随窗口**物理**尺寸（CSS 逻辑 × dpr）：微信把画布 fill 铺满窗口，画布若与窗口
  // 比例不一致就会被**非等比拉伸**（开发者工具预览 1380×652 下横 1.08×、纵 0.91×，
  // 画面拉伸 + 字体发糊的直接根因）。渲染端（web-runtime.js drawScene）把 1280×720
  // 设计画布等比 contain 居中绘制（黑边），触摸也做同一逆变换（clientX → 设计坐标），
  // 控件命中不受影响。`VIEW_WIDTH/VIEW_HEIGHT` 仍是**设计声明**（诊断/比对 project.toml 用）。
  canvas.width = Math.max(1, Math.round(screenWidth * pixelRatio));
  canvas.height = Math.max(1, Math.round(screenHeight * pixelRatio));
  // 窗口尺寸变化（开发者工具拖动预览窗口 / 真机横竖屏切换）时重新跟随：
  // 画布与窗口比例一旦失配，宿主 fill 就会重新出现非等比拉伸。runtime 的 drawScene/
  // 输入逆变换每帧都从 canvas.width/height 现算，无需再通知。
  if (typeof wx.onWindowResize === 'function') {
    wx.onWindowResize(function (res) {
      if (res && res.windowWidth > 0 && res.windowHeight > 0) {
        screenWidth = res.windowWidth;
        screenHeight = res.windowHeight;
        canvas.width = Math.max(1, Math.round(screenWidth * pixelRatio));
        canvas.height = Math.max(1, Math.round(screenHeight * pixelRatio));
        // P1 第二片：宿主接管画面之后，窗口变化必须**通知宿主**——surface 的 config、渲染目标、
        // 取景矩形与像素回读尺寸是四件必须同一帧改完的事，而那一处实现只在 wasm 里
        // （engine_host_resize）。只改 canvas 属性会让画面按旧尺寸渲、往新尺寸显（比例永久失配，
        // 而 presented= / rames= 全都正常，属于读数看不出来的错）。
        // 失败只告警不掀翻：resize 不该把启动或帧循环带崩。
        if (engineHost.status === 'running' && engineHost.visibleTakeover
          && engineHostRef && typeof engineHostRef.engine_host_resize === 'function') {
          try {
            engineHostRef.engine_host_resize(canvas.width, canvas.height);
            engineHost.resizeCalls += 1;
          } catch (error) {
            warn('wechat-adapter: engine_host_resize 失败 -> ' + hostError(error));
          }
        }
        // 2d 改道那张离屏画布也跟着改：它虽然不上屏，但旧世界仍按它的尺寸算布局与命中，
        // 停在旧尺寸会留下「窗口变了之后旧世界命中框错位」这种只在接管形态才有的错。
        if (redirectCanvas) {
          try {
            redirectCanvas.width = canvas.width;
            redirectCanvas.height = canvas.height;
          } catch (error) { /* 改不动不影响结论 */ }
        }
      }
    });
  }
  // 宿主 canvas 通常允许挂 own property（和 Image.src 同一套社区做法），但代理/冻结对象上
  // 赋值同样会**静默失败**——那样运行时的 canvas.addEventListener 就是 undefined，输入会
  // 整条失效而没有任何报错。这里回读校验，挂不上就换一个只做事件转发的包装对象
  // （getContext/width/height 仍转发到真 canvas，绘制不受影响）。
  function ensureCanvasEventTarget(target) {
    createEventTarget(target);
    if (typeof target.addEventListener === 'function' && typeof target.dispatchEvent === 'function') return target;
    warn('wechat-adapter: 宿主 canvas 不允许挂载事件监听，改用转发包装对象（绘制仍走真 canvas）');
    var wrapper = {
      getContext: function (kind) { return target.getContext(kind); },
      style: target.style || {}
    };
    ['width', 'height'].forEach(function (key) {
      try {
        Object.defineProperty(wrapper, key, {
          configurable: true,
          get: function () { return target[key]; },
          set: function (value) { target[key] = value; }
        });
      } catch (e) { /* 属性描述失败时下面的校验会报出来 */ }
    });
    createEventTarget(wrapper);
    if (typeof wrapper.addEventListener !== 'function' || typeof wrapper.getContext !== 'function') {
      throw new Error('wechat-adapter: 无法为 canvas 建立事件通道（宿主 canvas 既不能挂载监听，包装也失败）');
    }
    return wrapper;
  }

  var canvasTarget = ensureCanvasEventTarget(canvas);
  canvasTarget.getBoundingClientRect = function () {
    return {
      left: 0, top: 0,
      width: canvas.width, height: canvas.height,
      right: canvas.width, bottom: canvas.height
    };
  };

  var gameFontFamily = null;

  function wrapContext(ctx) {
    if (!ctx) return ctx;
    // 运行时把字体写成 'RustLuaGameFont, sans-serif'；小游戏里真实可用的是
    // wx.loadFont 返回的字体族名，这里在 ctx.font 上做一次等价替换。
    var stored = 'sans-serif';
    try {
      Object.defineProperty(ctx, 'font', {
        configurable: true,
        enumerable: true,
        get: function () { return stored; },
        set: function (next) {
          var value = String(next);
          if (gameFontFamily) value = value.replace('RustLuaGameFont', gameFontFamily);
          stored = value;
        }
      });
    } catch (e) {
      warn('wechat-adapter: 无法接管 ctx.font，中文将退回默认字体（' + e + '）');
    }
    return ctx;
  }

  // 2d 改道的离屏画布（只在接管形态下创建，第一次 '2d' 请求时）。尺寸取**当时**主画布的
  // 尺寸快照；`onWindowResize` 之后主画布变了、这张不会跟着变——这片是验收通道，记在
  // twoDRedirect.note 之外不再处理（resize 在模拟器验收里不发生）。
  function ensureTwoDRedirectCanvas() {
    if (redirectCanvas) return redirectCanvas;
    var width = Math.max(1, canvas.width || VIEW_WIDTH);
    var height = Math.max(1, canvas.height || VIEW_HEIGHT);
    if (typeof wx.createOffscreenCanvas === 'function') {
      try {
        var off = wx.createOffscreenCanvas({ type: '2d', width: width, height: height });
        if (off && typeof off.getContext === 'function') {
          redirectCanvas = off;
          twoDRedirect.source = 'wx.createOffscreenCanvas(2d,' + width + 'x' + height + ')';
          return off;
        }
      } catch (error) { twoDRedirect.note = twoDRedirect.note || 'offscreen-2d-threw'; }
    }
    // 兜底：`wx.createCanvas()` 首次之外的调用返回离屏画布（主画布已在上面拿过）。
    var again = null;
    try { again = safeCall(wx, 'createCanvas'); } catch (error) { again = null; }
    if (again && again !== canvas && typeof again.getContext === 'function') {
      again.width = width;
      again.height = height;
      redirectCanvas = again;
      twoDRedirect.source = 'wx.createCanvas(2d-fallback,' + width + 'x' + height + ')';
      return again;
    }
    twoDRedirect.status = 'failed';
    twoDRedirect.note = twoDRedirect.note || 'no-redirect-canvas';
    return null;
  }

  var rawGetContext = canvas.getContext;
  canvas.getContext = function (kind, attributes) {
    if (kind !== '2d') return rawGetContext ? rawGetContext.call(canvas, kind, attributes) : null;
    if (!canvas.__rustluaContext) {
      // 第二个参数必须转给宿主：运行时用 `{ willReadFrequently: true }` 声明"这层 canvas 要频繁
      // getImageData 回读像素"（画面自检），丢了它宿主就按"很少回读"配上下文，开发者工具会为
      // 每次回读提示一次性能警告。
      var ctx = null;
      if (engineVisibleTakeover) {
        var redirect = ensureTwoDRedirectCanvas();
        // 改道成功才走离屏；拿不到改道画布时**退回主画布**（画面优先于接管——宁可验收降级，
        // 不许黑屏）。退回去后宿主在主画布上拿不到 webgl2，会在 engine_host.reason 里具名失败。
        if (redirect) {
          ctx = redirect.getContext('2d', attributes);
          if (ctx) twoDRedirect.status = 'redirected';
        }
        if (!ctx) twoDRedirect.status = 'fallback-main';
      }
      if (!ctx) ctx = rawGetContext ? rawGetContext.call(canvas, '2d', attributes) : null;
      if (!ctx) return null;
      canvas.__rustluaContext = wrapContext(ctx);
      log('wechat-adapter: 2d 上下文绑定 path=' + (twoDRedirect.status === 'redirected' ? 'offscreen-redirect' : 'main-canvas') +
        ' v=' + ADAPTER_VERSION);
    }
    return canvas.__rustluaContext;
  };

  var bootAttributes = {};
  var statusElement = { textContent: '', style: {} };
  var bodyElement = {
    setAttribute: function (name, value) { bootAttributes[String(name)] = String(value); },
    getAttribute: function (name) {
      return Object.prototype.hasOwnProperty.call(bootAttributes, name) ? bootAttributes[name] : null;
    },
    removeAttribute: function (name) { delete bootAttributes[name]; },
    appendChild: function () {}, style: {}, textContent: ''
  };
  // ④ 前后台的读数底座。引擎宿主的 `LifecycleEvents`（game_web/src/gpu_host.rs）只认两件事：
  // `document.hidden` 的实时值 + `visibilitychange` 订阅。这一版之前 addEventListener 是 no-op、
  // 且没有 `hidden`——微信侧生命周期与引擎从此**互不相通**。改成真事件注册表（createEventTarget）
  // 加 getter，由 installLifecycle 在 wx.onHide/onShow 里写旗并派发。
  var documentHidden = false;
  var documentShim = {
    readyState: 'complete',
    body: bodyElement,
    documentElement: bodyElement,
    fonts: { add: function () {} },
    querySelectorAll: function () { return []; },
    querySelector: function () { return null; },
    createElement: function (tag) {
      return { tagName: String(tag), style: {}, setAttribute: function () {}, appendChild: function () {} };
    },
    getElementById: function (id) {
      if (id === 'rustlua-canvas') return canvasTarget;
      if (id === 'status') return statusElement;
      return null;
    }
  };
  createEventTarget(documentShim);
  try {
    Object.defineProperty(documentShim, 'hidden', {
      get: function () { return documentHidden; },
      enumerable: true,
      configurable: true
    });
  } catch (error) {
    documentShim.hidden = documentHidden;
    warn('wechat-adapter: document.hidden 装不上 getter（退化为定值，前后台读数不可信）-> ' + hostError(error));
  }

  function bootMark(name) {
    if (bodyElement.getAttribute(name) != null) return true;
    return false;
  }

  // 适配层**自己**画的每一笔都要走这个包装。原因不是洁癖：无头 harness 用
  // "每帧绘制数严格递增"来证明帧循环真的在画东西，而适配层的进度条/警示条/覆盖层会让
  // 某帧之后每帧多画或少画几笔，把一条正确的断言打成假红。标记写在 ctx 上，harness 据此
  // 把适配层的笔数排除在外——**不能**反过来把断言放宽成 "ops > 0"（那等于放弃这条证据）。
  // 顺带承担"自绘失败绝不能影响游戏帧"：任何异常在这里被吞掉并降级为告警。
  function withAdapterDraw(ctx, fn) {
    if (!ctx) return undefined;
    var previous = ctx.__rustluaAdapterDraw;
    ctx.__rustluaAdapterDraw = true;
    try { return fn(ctx); }
    catch (error) { warn('wechat-adapter: 适配层自绘失败 -> ' + hostError(error)); return undefined; }
    finally { ctx.__rustluaAdapterDraw = previous === undefined ? false : previous; }
  }

  // 启动失败时把原因画到屏幕上：小游戏没有 DOM 控制台，真机上只能靠画面自证。
  function showBootError(message) {
    try {
      var ctx = canvas.getContext('2d');
      if (!ctx) return;
      ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = '#151923';
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = '#ff8a80';
      ctx.font = '22px sans-serif';
      var text = String(message);
      var max = Math.floor(canvas.width / 12);
      var lines = [];
      for (var i = 0; i < text.length && lines.length < 12; i += max) lines.push(text.slice(i, i + max));
      for (var j = 0; j < lines.length; j++) ctx.fillText(lines[j], 24, 60 + j * 30);
      // 启动失败画面也要带"贴图失败"警示条：正常帧循环没跑起来时（启动失败）那条常驻警示条
      // 不会被调用，而这一刻恰恰最需要屏幕说清"素材为什么没出来"。
      var imageFailures = (assetStats.imageFailed || 0) + (assetStats.imageMissing || 0);
      if (imageFailures > 0) {
        bandFailures = imageFailures;
        // 这一笔发生在 boot 失败路径上（没有帧循环），不存在"每帧笔数"口径问题，
        // 但仍然走同一个包装：语义统一，且自绘异常不会顶掉真正的失败原因。
        withAdapterDraw(ctx, drawFailureBand);
      }
    } catch (e) {
      warn('wechat-adapter: 错误提示绘制失败 ' + e);
    }
  }

  // ---------------------------------------------------------------- 渲染自证
  //
  // 小游戏宿主里没有 DOM 控制台，真机/开发者工具出问题时只能靠**画面自证**：
  // "流程正常但屏幕全黑" 这种最坏情况（贴图静默丢失、画到了离屏画布、上下文被包装坏）
  // 在日志里和成功长得一模一样。所以这里提供两件事：
  //   1) `__RUSTLUA_DIAG()`：一份可复制的 JSON 快照（主画布来源、像素回读、帧数、贴图、远端）；
  //   2) 可开启的屏幕覆盖层：四角色块 + 对角线 + 诊断文字，每帧画在游戏画面**之上**。
  //      开启方式（任一）：发布目录里有 `debug-overlay.txt`，或 wx.getStorageSync('rustlua_debug_overlay')。
  // 另外，一旦贴图真的加载失败（宿主 onerror 说话），屏幕上会**始终**出现红色警示条——
  // 宁可画面难看，也不要"少了一半素材却没有任何提示"。
  var DEBUG_OVERLAY_FILE = 'debug-overlay.txt';
  var debugOverlayEnabled = false;
  var frameCount = 0;
  var overlayFrames = 0;
  var pixelProbeValue = 'not-run';
  // 是否在帧循环之前就画过一次覆盖层：用来区分「开关没生效」与「根本没有帧」。
  var probeAtStart = false;
  var ctxKind = 'unknown';
  var bandFailures = 0;
  // Image.src 到底是从哪一层接管的（own-accessor / prototype-accessor / setSrc-method / none）。
  // 这一项是"贴图交给了宿主、宿主却没回调"这类黑屏的唯一现场凭据，必须能读到。
  var srcTakeover = '';
  // onload/onerror 是从哪一层接管的（host-setter / own-accessor）。与 src 同源的第二条接管契约：
  // 宿主把回调桥接在**私有字段**上时（基础库 `k$` 包装类），用自己的 accessor 盖住就等于
  // 把桥拆了——贴图回调永远传不回运行时，症状同样是"卡死 + 0 错误"。
  var callbackTakeover = '';

  function debugOverlayRequested() {
    // quiet=true：没有这个文件是常态，不该刷"读取失败"的日志。
    var probe = readWithStrategies(DEBUG_OVERLAY_FILE, readTextRaw, true);
    if (probe && probe.value) return true;
    var stored = safeCall(wx, 'getStorageSync', 'rustlua_debug_overlay');
    return !!(stored && stored !== '' && stored !== '0' && stored !== 'false');
  }

  // 唯一能证明"画上去的东西真的落在上屏画布上"的办法：画一小块，再读回像素。
  function probePixels(ctx) {
    if (!ctx || typeof ctx.fillRect !== 'function' || typeof ctx.getImageData !== 'function') {
      return 'unsupported（ctx 没有 fillRect/getImageData）';
    }
    try {
      ctx.save && ctx.save();
      ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = '#ff00ff';
      ctx.fillRect(1, 1, 6, 6);
      var data = ctx.getImageData(1, 1, 1, 1);
      ctx.restore && ctx.restore();
      if (!data || !data.data || data.data.length < 4) return 'no-data';
      var rgba = [data.data[0], data.data[1], data.data[2], data.data[3]];
      var hit = rgba[0] > 200 && rgba[1] < 60 && rgba[2] > 200;
      return rgba.join(',') + (hit ? '（命中：绘制确实落在上屏画布上）' : '（不符：#ff00ff 没有读回来）');
    } catch (error) {
      return 'err:' + hostError(error);
    }
  }

  function bootAttr(name) {
    // 必须和 web-runtime.js 的取值顺序一致：`document.body || document.documentElement`
    // （宿主只补上 documentElement 时，标记写在它上面；读 body 会读到空，于是诊断"报 0"
    // 而画面其实是好的——诊断撒谎比没有诊断更糟）。
    var target = documentTarget || null;
    var body = (target && (target.body || target.documentElement)) || bodyElement;
    var value = null;
    try { if (body && typeof body.getAttribute === 'function') value = body.getAttribute(name); } catch (e) { value = null; }
    if (value === null || value === undefined) value = bodyElement.getAttribute(name);
    return value;
  }

  // 标记到底写在哪个对象上（none = 运行时的标记读不到，此时 drawn/frames 之外的自证以
  // 像素回读为准）。
  function bootMarkTargetName() {
    var target = documentTarget || null;
    if (!target) return 'none';
    if (target.body === bodyElement || target.documentElement === bodyElement) return 'adapter-body';
    if (target.body) return 'host-body';
    if (target.documentElement) return 'host-documentElement';
    return 'none';
  }

  function diagSnapshot() {
    var stats = assetStats;
    var loaded = stats.imageLoaded || 0;
    var failed = stats.imageFailed || 0;
    return {
      canvas_source: canvasSource || 'unknown',
      canvas_is_main: canvasIsMain,
      canvas_is_created: canvasIsCreated,
      canvas_size: canvas.width + 'x' + canvas.height,
      canvas_style: (canvas.style && canvas.style.width) ? (canvas.style.width + 'x' + canvas.style.height) : 'none',
      screen: screenWidth + 'x' + screenHeight + '@' + pixelRatio,
      ctx: ctxKind,
      // 微信侧 GL 能力（WP8 的设备验证仪器）：`probed=false` 且 `note=probe-canvas-is-main`
      // 表示"宿主只肯给已经用作 2d 的那张画布"，此时**故意不探测**——一张画布只能有一个上下文。
      gl: probeGlCapability(),
      // 引擎侧画布探针（WP8 第二片）：`off` = 开关没开；`unavailable` = 内核没编进 GPU 路径；
      // `ok`/`failed` = 真的问过引擎（`failed` 里带 wasm 侧的具名原因）。默认关 ⇒ 生产读数恒为 `off`。
      engine_probe: {
        status: engineProbe.status,
        source: engineProbe.source || 'none',
        reason: engineProbe.reason || '',
        reports: engineProbe.reports.slice(0, 4)
      },
      // 引擎宿主（WP8 主线第二片）：与探针分开报——探针回答"能不能起渲染栈"，宿主回答
      // "起来之后每帧真的出了多少帧/走了多少固定步"。`running` 才有 frames/presented 的增量。
      // **本笔之后默认形态是原子模式**：不带任何开关时这里会是 running（真机）或 failed
      // （离线假宿主不给 GL）——`off` 只在原子模式被显式关掉时才出现。
      engine_host: {
        status: engineHost.status,
        source: engineHost.source || 'none',
        reason: engineHost.reason || '',
        begin: engineHost.begin || '',
        frames: engineHost.frames,
        steps: engineHost.steps,
        presented: engineHost.presented,
        failed_presents: engineHost.failed,
        clamped_frames: engineHost.clamped,
        input_applied: engineHost.inputApplied,
        // WP7-55：定长帧 ABI + "谁在跑固定步"。
        abi: engineHost.abi || 'none',
        abi_version: engineHost.abiVersion,
        tick: engineHost.tick ? 'lua' : 'none',
        tick_conflict: engineHost.tickConflict,
        sim_steps: engineHost.simSteps,
        tick_failures: engineHost.tickFailures,
        runtime_fixed_frames: engineHost.runtimeFixedFrames,
        runtime_run_steps: engineHost.runtimeRunSteps,
        runtime_input_frames: engineHost.runtimeInputFrames,
        hidden_skips: engineHost.hiddenSkips,
        mem_begin: engineHost.memBegin,
        // 方案 A 的两条形态读数：宿主绑的是不是主画布 / 2d 有没有改道成功。
        // 两条必须一起看——`visible_takeover=true` 而 `two_d_redirect=off` 意味着 2d 先于
        // 开关绑上了主画布（harness 置开关的时机错了），那时宿主拿不到 webgl2。
        visible_takeover: engineHost.visibleTakeover,
        scene_pump: engineHostScenePump,
        // 帧源（谁拥有帧泵）：`wx.requestAnimationFrame` / `setTimeout` / `none`（泵没起来）。
        // 与 `scene_pump` 一起读：`scene_pump=true` 而它是 `none` 是不可能的组合，出现即说明
        // "泵标记被置上但一次都没调度"。
        frame_source: engineHost.frameSource || 'none',
        // 成功通知宿主的窗口尺寸变化次数（见引擎宿主对象上 resizeCalls 的说明）：
        // 与 size 一起读——size 是宿主**以为**的尺寸，这个数是它**收到过**几次变化。
        resize_calls: engineHost.resizeCalls,
        // B4：宿主自己的输入通道（见引擎宿主对象上这几个字段的说明）。与 `runtime_input_frames`
        // 一起读才判得出交接：前者恒 0 且后者增长 = 还没交接；两者都恒 0 = 两个都没动。
        host_input_frames: engineHost.hostInputFrames,
        host_input_dropped_keys: engineHost.hostInputDroppedKeys,
        host_input_last: engineHost.hostInputLast,
        // 多指：`host_touch_events > 0` 证明**真手指**到了每指导出；`host_touch_multi > 0` 才是
        // "两指同时按"（需要用例真的同时按住两根手指，单指用例只会让前者 > 0）。
        host_touch_events: engineHost.hostTouchEvents,
        host_touch_multi: engineHost.hostTouchMulti,
        two_d_redirect: {
          status: twoDRedirect.status,
          source: twoDRedirect.source || 'none',
          note: twoDRedirect.note || ''
        },
        last_state: engineHost.lastState || null
      },
      // ④ 前后台底座读数：hidden 旗的当前值 + 谁在订 visibilitychange + 派发目标到底是谁。
      // `subs=-1` = documentTarget 没有 __listenerCount（宿主 document 抢占且补不上注册表）。
      // run20 教训：`subs=0` 而探针明明订阅成功了——说明**探针订的对象**和**适配层派发的
      // 对象**可能不是同一个（宿主 document 在 boot 期间被换/代理）。没有这三条身份读数，
      // 事件到达与否的失败现场无法归因。
      visibility: {
        hidden: documentHidden,
        subs: (typeof documentTarget !== 'undefined' && documentTarget &&
          typeof documentTarget.__listenerCount === 'function')
          ? documentTarget.__listenerCount('visibilitychange') : -1,
        target_is_shim: (typeof documentTarget !== 'undefined') && documentTarget === documentShim,
        global_matches_target: g.document === documentTarget,
        target_ctor: (documentTarget && documentTarget.constructor && documentTarget.constructor.name) || 'none'
      },
      pixel_probe: pixelProbeValue,
      frames: frameCount,
      version: ADAPTER_VERSION,
      boot: bootAttr('data-rustlua-boot') || 'unknown',
      boot_detail: String(bootAttr('data-rustlua-boot-detail') || '').slice(0, 120),
      boot_marks: bootMarkTargetName(),
      // `off` 而不是 `0`：这几个读数由运行时的诊断开关决定写不写（web-runtime.js 的
      // DIAGNOSTICS_ON，发布默认关）。把它们缺省成 0 会让现场报告看起来像"启动了但什么都没画"
      // ——那正好是这套诊断要区分的故障，不能由诊断自己伪造出来。
      drawn: bootAttr('data-rustlua-drawn') || 'off',
      ui_elements: bootAttr('data-rustlua-ui-elements') || 'off',
      world: String(bootAttr('data-rustlua-world') || '').slice(0, 200) || 'off',
      timing: bootAttr('data-rustlua-timing') || 'off',
      // 输入批量协议（`RLFI`）的落地读数，**常驻**（运行时那三个数是 JS 侧计数器、不额外跨界）。
      // 微信侧看不到浏览器 DevTools，这一条是"输入到底有没有送到 Lua"的现场唯一凭据：
      // `bits` 恒为 0 = 宿主与 wasm 的位语义不一致；`unmapped` 非空 = 键名落不进线路表。
      input: bootAttr('data-rustlua-input') || 'off',
      document_shim: shimReports.document ? {
        replaced: shimReports.document.replaced,
        patched: shimReports.document.patched,
        blocked: shimReports.document.blocked,
        missing: shimReports.document.missing
      } : null,
      window_shim: shimReports.window ? { replaced: shimReports.window.replaced, blocked: shimReports.window.blocked, missing: shimReports.window.missing } : null,
      images: 'ok=' + loaded + ' fail=' + failed + ' 未供应=' + (stats.imageMissing || 0) +
        (stats.imageCacheLoadFailed ? '（其中 ' + stats.imageCacheLoadFailed + ' 张来自 USER_DATA_PATH 缓存）' : ''),
      image_src_takeover: srcTakeover || 'none',
      image_callback_takeover: callbackTakeover || 'none',
      // wasm 核心状态：`ok` 才算真的装进来了；其它取值都能指出是哪一层不对（见 loadWasmCoreOnce）。
      // hostApi：WP10 微信侧接线状态（ok=HostApi 已实例化挂到 __RUSTLUA_HOST_API）。
      wasm: {
        status: wasmState.status,
        source: wasmState.source,
        channel: wasmState.channel,
        path: wasmState.path,
        // 标准 `WebAssembly` 域名的来源：facade=转发过来的宿主构造器，host-standard=宿主自己就有。
        facade: wasmState.facade,
        bytes: wasmState.bytes,
        exports: wasmState.exports,
        detail: wasmState.detail,
        attempts: wasmState.attempts,
        // EH 真机探针读数（并发跑、独立于 status）：`eh_probe=accepted` 说明设备引擎实现 wasm EH、
        // 剥完 tag 导入的产物（含本地 $tag$0 + legacy try/catch）能落地；`reject[...]`/`timeout` 才是
        // "得换不用 EH 的产物形态"的信号。见 wasm-bridge.js 的 probeEHAcceptance。
        ehProbe: ehProbeReadout,
        luaBridge: wasmState.luaBridge,
        hostApi: wasmState.hostApi
      },
      // 画布 vs 工程声明尺寸（横版方案A）：agree=false 就是"版面会溢出画布"。
      view_agreement: viewAgreement,
      read_strategy: preferredStrategy.pack || '(未探明)',
      // 宿主 FS 的路由包装状态：hits=0 而产物是分包形态 ⇒ 说明这次启动没有任何读者走路由
      // （首屏读取全靠适配层自己的通道，或者清单没读到），与 `layout.entries` 一起看。
      fs_route: fsRouteInfo(),
      remote: remoteConfig ? (remoteConfig.packBaseUrl + ' remote=' + stats.remote + ' failed=' + stats.remoteFailed) : 'off',
      image_samples: (stats.imageFailureSamples || []).slice(0, 4),
      // 发布形态（主包 / 分包）、首屏清单规模、以及有多少次读取真的走了主包 mainpack/。
      // 分包形态的形态自证全靠这三个数：form=none 说明清单没读到（路由没生效），
      // mainpack_reads=0 说明首屏资源一次都没从主包读（清单漏列或路由失效）。
      layout: {
        form: firstScreen ? 'mainpack+pack' : 'none',
        manifest: MAINPACK_MANIFEST,
        entries: firstScreen ? firstScreen.count : 0,
        mainpack_reads: mainpackReads
      },
      pack: {
        // declared 来自首屏清单（不是 game.json，见 packSubpackageName）；attempted 是"真的去挂过吗"。
        // 这两个数**必须同时看**：declared=true 而 attempted=false 就是最危险的那种失败——
        // 判据说该挂、却一次都没去挂，屏幕上不会有任何提示（分包永远不会就绪）。
        declared: hasPackSubpackage(),
        name: packSubpackageName(),
        attempted: packMountAttempts > 0,
        state: packState,
        progress: packProgress,
        progress_events: packProgressEvents,
        mount_frame: packMountFrame,
        failure: packFailure ? packFailure.slice(0, 160) : ''
      },
      // 代码包后缀白名单的改名情况：map_entries=0 说明 code-package.json 没读到
      // （真机上 `.lua` 会按原名去读 → 一个脚本都加载不了），lookups_hit 是"查表命中过几次"。
      upload_rules: renamingInfo(),
      overlay: debugOverlayEnabled,
      probe_at_start: probeAtStart,
      probe_before_frames: probeAtStart && frameCount === 0
    };
  }

  // `data-rustlua-world` 是运行时每帧自报的世界状态（地图/实体数/落点/主角坐标与当前动画）。
  // 覆盖层把它翻成两行短句：开发者工具里"游戏在跑但世界是空的"必须能一眼看出来，
  // 而不是只能靠贴图数量去猜。渲染侧坐标与 Lua 世界并列显示，两者不一致就是回写通道断链。
  function worldLines(raw) {
    var text = String(raw || '');
    if (!text) return [];
    var field = function (name) {
      var match = text.match(new RegExp(name + '=([^\\s]*)'));
      return match ? match[1] : '';
    };
    var lines = ['世界 地图=' + (field('map') || '(空)') + ' 实体=' + (field('alive') || '?') + '/' + (field('scene') || '?') +
      ' 落点=' + (field('spawn') || 'none')];
    var player = text.match(/player=(\S+?)@(-?\d+),(-?\d+)(?:\/(\S+))?/);
    if (player) {
      var drawn = text.match(/drawn=(\S+?)@(-?\d+),(-?\d+)/);
      lines.push('主角 ' + player[1] + '@' + player[2] + ',' + player[3] + (player[4] ? ' 动画=' + player[4] : '') +
        ' 渲染侧=' + (drawn ? drawn[2] + ',' + drawn[3] : '?'));
    }
    return lines;
  }

  function diagText() {
    var d = diagSnapshot();
    var lines = [
      'RustLua DIAG 覆盖层（debug-overlay.txt / rustlua_debug_overlay 已开启）',
      '主画布 ' + d.canvas_size + ' 来源=' + d.canvas_source + ' 上屏=' + (d.canvas_is_main ? 'yes' : 'NO') +
        ' 首调=' + (d.canvas_is_created ? 'yes' : 'no') + ' 样式=' + d.canvas_style,
      '屏幕 ' + d.screen + ' ctx=' + d.ctx + ' 像素回读=' + d.pixel_probe,
      '帧=' + d.frames + ' 绘制=' + d.drawn + ' 标记写于=' + d.boot_marks + ' UI=' + d.ui_elements + ' boot=' + d.boot,
      '贴图 ' + d.images + ' src接管=' + d.image_src_takeover + ' 回调接管=' + d.image_callback_takeover +
        ' 读取形式=' + d.read_strategy,
      '远端 ' + d.remote,
      '读取策略 main=' + (preferredStrategy.main || '?') + ' pack=' + (preferredStrategy.pack || '?'),
      '发布形态 ' + d.layout.form + ' 首屏清单=' + d.layout.entries + ' 条 主包读取=' + d.layout.mainpack_reads +
        ' 次 分包声明=' + (d.pack.declared ? '是(' + d.pack.name + ')' : '否') +
        ' 已尝试挂载=' + (d.pack.attempted ? '是' : '否') + ' 分包状态=' + d.pack.state +
        (d.pack.state === 'loading' ? '(' + d.pack.progress + '%)' : '') +
        (d.pack.mount_frame >= 0 ? ' 挂载帧=' + d.pack.mount_frame : '')
    ];
    if (d.boot_detail) lines.push('detail=' + d.boot_detail);
    return lines.concat(worldLines(d.world));
  }

  // 启动完成后立刻画一次自证图案（四角 + 对角线 + 诊断面板），**不依赖帧循环**。
  // 为什么必须有一次脱离帧循环的留痕：真机上最常见的黑屏是"启动流程走完了、一帧都没画"，
  // 那时如果只在帧回调里画覆盖层，屏幕上什么都看不到，也就无法区分
  // 「覆盖层没生效」/「没有帧」/「画到了看不见的画布」。实测还踩过一次：放在 boot 之前画，
  // 会被 boot 自己的清屏擦掉，等于没画——所以位置必须在 boot 之后。
  function drawStartupProbe() {
    if (!debugOverlayEnabled) return false;
    try {
      var ctx = canvas.getContext('2d');
      if (!ctx) { warn('wechat-adapter: 启动后自证绘制跳过（canvas 拿不到 2d ctx）'); return false; }
      pixelProbeValue = withAdapterDraw(ctx, probePixels);
      withAdapterDraw(ctx, drawOverlay);
      probeAtStart = true;
      log('wechat-adapter: 启动后自证绘制完成（不依赖帧循环）' + JSON.stringify(diagSnapshot()));
      publishDiag();
      return true;
    } catch (error) {
      warn('wechat-adapter: 启动后自证绘制失败 -> ' + hostError(error));
      return false;
    }
  }

  function drawOverlay(ctx) {
    var lines = diagText();
    ctx.save && ctx.save();
    ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    var w = canvas.width;
    var h = canvas.height;
    // 四角色块 + 对角线：哪怕游戏一帧都没画，这些也必须出现在屏幕上。
    ctx.fillStyle = '#ff0055';
    ctx.fillRect(0, 0, 56, 56);
    ctx.fillRect(w - 56, 0, 56, 56);
    ctx.fillRect(0, h - 56, 56, 56);
    ctx.fillRect(w - 56, h - 56, 56, 56);
    ctx.strokeStyle = '#00ff88';
    ctx.lineWidth = 12;
    ctx.beginPath();
    ctx.moveTo(56, 56);
    ctx.lineTo(w - 56, h - 56);
    ctx.stroke();
    ctx.fillStyle = 'rgba(0,0,0,0.72)';
    ctx.fillRect(0, 0, w, 26 + lines.length * 26);
    ctx.font = '20px sans-serif';
    for (var i = 0; i < lines.length; i++) {
      ctx.fillStyle = i === 0 ? '#7cf7c4' : '#e8f0ff';
      ctx.fillText(lines[i], 12, 30 + i * 26);
    }
    ctx.restore && ctx.restore();
  }

  // 贴图加载失败时的常驻警示条：把"静默变黑"变成屏幕上的可见事实。
  function drawFailureBand(ctx) {
    ctx.save && ctx.save();
    ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    var w = canvas.width;
    ctx.fillStyle = 'rgba(160,0,0,0.85)';
    ctx.fillRect(0, 0, w, 34);
    ctx.fillStyle = '#fff';
    ctx.font = '20px sans-serif';
    ctx.fillText('贴图加载失败 ' + bandFailures + ' 张（画面会缺素材；检查 CDN 可达性与域名校验）', 12, 25);
    ctx.restore && ctx.restore();
  }

  // 分包下载进度条。微信端**根本没有 loading UI**（statusElement 是个 no-op 对象），
  // 而分包是几十 MB 量级的下载——没有这条进度，玩家看到的就是"标题出来后卡住不动"。
  // 只在真的在下载时画（state=loading）；挂载成功后立刻停画，把画布还给游戏。
  function drawLoadingBar(ctx) {
    var w = canvas.width;
    var h = canvas.height;
    var barWidth = Math.round(w * 0.6);
    var barHeight = 14;
    var left = Math.round((w - barWidth) / 2);
    var top = h - 56;
    ctx.save && ctx.save();
    ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.fillStyle = 'rgba(0,0,0,0.55)';
    ctx.fillRect(left - 6, top - 30, barWidth + 12, barHeight + 44);
    ctx.fillStyle = '#e8f0ff';
    ctx.font = '20px sans-serif';
    var label = packProgress > 0 ? '正在下载资源 ' + packProgress + '%' : '正在下载资源…';
    ctx.fillText(label, left, top - 8);
    ctx.fillStyle = 'rgba(255,255,255,0.25)';
    ctx.fillRect(left, top, barWidth, barHeight);
    ctx.fillStyle = '#7cf7c4';
    ctx.fillRect(left, top, Math.round(barWidth * Math.max(0, Math.min(100, packProgress)) / 100), barHeight);
    ctx.restore && ctx.restore();
  }

  // 分包载入失败横幅。这句话**按形态分两套**，因为失败的后果不一样：
  //   · 内核在主包（full / remote）：标题画面照常出，只有玩法场景缺素材 ⇒ 一条红条提示；
  //   · 内核在分包（subpackage）：Lua 执行层就在没挂上的那个文件里，boot 必然失败 ⇒
  //     必须说清"是这次进不去、重进可以"，而不是让玩家以为游戏还能跑（旧文案在这形态是谎话）。
  function drawPackFailure(ctx) {
    var w = canvas.width;
    var h = canvas.height;
    ctx.save && ctx.save();
    ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.fillStyle = 'rgba(120,0,0,0.9)';
    ctx.fillRect(0, 38, w, 34);
    ctx.fillStyle = '#fff';
    ctx.font = '20px sans-serif';
    ctx.fillText(isWasmInPack()
      ? '分包（pack）加载失败：游戏内核就在这个分包里，本次启动无法继续；重进即可（下载一次永久缓存）'
      : '分包（pack）加载失败：进玩法场景会缺素材；游戏从主包照常启动', 12, 63);
    ctx.restore && ctx.restore();
  }

  // boot 前重试期间的横幅。与失败条同一条位置、不同颜色：这一刻**还没有失败**，红条会让玩家
  // 以为游戏坏了（而进度条还在动，两者同时出现只会被读成"系统在骗人"）。
  function drawPackRetryNotice(ctx) {
    var w = canvas.width;
    ctx.save && ctx.save();
    ctx.setTransform && ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.fillStyle = 'rgba(90,60,0,0.9)';
    ctx.fillRect(0, 38, w, 34);
    ctx.fillStyle = '#fff';
    ctx.font = '20px sans-serif';
    ctx.fillText(packBootRetryNotice || '分包重试中…', 12, 63);
    ctx.restore && ctx.restore();
  }

  // 现场导出通道。为什么要三条：开发者工具控制台的"上下文"**未必就是游戏全局**——实测
  // 日志明明打了 `适配层已就绪 v…（__RustLuaWechat / __RUSTLUA_DIAG 已注册）`，控制台里
  // `__RUSTLUA_DIAG()` 却报 "is not defined"（`__RustLuaWechat` 同样不可见）。所以除了挂全局，
  // 再加两条**不依赖全局作用域**的通道：
  //   ① `wx.__rustluaDiag()` / `wx.__rustluaCanvasInfo()` —— 控制台里只要能看到 wx 就能调用；
  //   ② `wx.getStorageSync('rustlua_diag')` —— 连控制台都用不上，直接看开发者工具的"存储"面板。
  function publishDiag() {
    var text;
    try { text = JSON.stringify(diagSnapshot()); } catch (e) { return; }
    try {
      if (typeof wx !== 'undefined' && wx && typeof wx.setStorageSync === 'function') wx.setStorageSync('rustlua_diag', text);
    } catch (e) { /* 存储不可用不影响游戏 */ }
  }

  // 每帧回调之后调用：先计数（永远），再按需覆盖层/警示条。
  function afterFrame() {
    frameCount += 1;
    // 引擎宿主每帧驱动。**必须在下面的提前 return 之前**：那段"没什么要画就 return"是
    // 覆盖层/警示条的逻辑，与宿主出帧无关（宿主关了就是空操作）。
    if (engineHost.status === 'running' && !engineHostScenePump) engineHostDriveFrame();
    // 需要改写 Image.src 却改不动时，每帧重申 failed：`data-rustlua-boot` 的权威读者是**宿主 body**
    // （见 bootAttr 的取值顺序），运行时会在启动过程中往同一个元素写 running，
    // 只在标记那一刻写一次会被后写的盖掉（实测：写完仍读到 boot=running）。
    if (srcRewriteFatalMessage) restampSrcRewriteFatal();
    var needOverlay = debugOverlayEnabled;
    var imageFailures = (assetStats.imageFailed || 0) + (assetStats.imageMissing || 0);
    var needPackUi = packState === 'loading' || packState === 'failed';
    if (!needOverlay && imageFailures === 0 && !needPackUi) return;
    ctxKind = 'unknown';
    var ctx = null;
    try { ctx = canvas.getContext('2d'); } catch (e) { ctx = null; }
    if (!ctx) return;
    ctxKind = (typeof ctx.fillRect === 'function' && typeof ctx.drawImage === 'function') ? '2d-ok' : '2d-缺方法';
    // 固定 z 序（后画的盖住先画的）：**进度条 → 贴图警示条 → 分包失败条 → 覆盖层**。
    // 依据是信息优先级：覆盖层是诊断面板，被任何东西盖住就失去意义，永远最上；
    // 贴图警示条说的是"画面已经在缺素材"（比"还在下载"更严重）所以压在进度条之上；
    // 分包失败条排在进度条之上、贴图警示条之下；进度条只是等待提示，垫在最底下。
    // `data-rustlua-boot=failed` 时**不画进度条/失败条**：那一刻整屏属于 showBootError 的错误页，
    // 叠上去只会互相遮挡，把真正的失败原因糊掉。
    var bootFailed = String(bootAttr('data-rustlua-boot') || '') === 'failed';
    if (packState === 'loading' && !bootFailed) withAdapterDraw(ctx, drawLoadingBar);
    if (imageFailures > 0) {
      bandFailures = imageFailures;
      withAdapterDraw(ctx, drawFailureBand);
    }
    // 重试期间画琥珀条（`packBootRetryNotice` 非空），落定失败才画红条。
    if (packState === 'failed' && !bootFailed) {
      withAdapterDraw(ctx, packBootRetryNotice ? drawPackRetryNotice : drawPackFailure);
    }
    if (needOverlay) {
      if (overlayFrames === 0) {
        pixelProbeValue = withAdapterDraw(ctx, probePixels);
        log('wechat-adapter: 渲染自证 ' + JSON.stringify(diagSnapshot()));
        publishDiag();
      }
      overlayFrames += 1;
      withAdapterDraw(ctx, drawOverlay);
      if (overlayFrames % 60 === 0) {
        log('wechat-adapter: 渲染自证 ' + JSON.stringify(diagSnapshot()));
        publishDiag();
      }
    }
  }

  // boot **之前**的等待画面（分包形态专用）。为什么必须由适配层自己驱动：wasm 内核搬进 pack/ 之后，
  // "能不能进 Lua"取决于分包挂载，而下载期间运行时的帧循环根本还没启动 —— 没人调用 afterFrame，
  // 那条 `packState === 'loading'` 的进度条一次都不会执行，玩家看到的就是"点开游戏后纯白屏十几秒"
  // （微信侧没有任何原生 loading UI，statusElement 是 no-op）。
  //
  // **两条刻意的约束**（第一版两条都踩了，表现都是"启动 booted 却一帧不跑"）：
  // ① 驱动只用 `setTimeout`，**不借 `wx.requestAnimationFrame`**：那条队列是运行时的。适配层每帧
  //    往队尾自取一条，就在帧预算里**抢走游戏自己的帧**（harness 的泵每轮只 shift 一条、并把那条
  //    计成一帧）；`cancelAnimationFrame` 又不可靠——本仓库 harness 给的句号是"入队时的队列长度"，
  //    出队却用 `shift()`，句号与槽位从一开始就对不上，取消会 null 掉**别人**的回调（真机句号稳定，
  //    但这里没必要赌）。`setTimeout` 在两边都是可取消的真实定时器，仍配合标志位停：多跑一次空回调
  //    无害，取消失败也不会留下活循环。
  // ② 画的是等待画面自己那一套（进度条 / 重试条 / 失败条），**不调 `afterFrame`**：后者会给
  //    `frameCount` 加一，而 `frameCount` 正是分包挂载帧的读数来源（`packInfo.mount_frame`，harness
  //    拿它逐帧对比宿主记录的那次挂载）。让等待画面去推它，等于用被测代码改判据。
  var preBootUi = { active: false, ticks: 0, id: 0 };

  function drawPreBootWait() {
    // boot 已判 failed ⇒ 整屏属于错误页，等待画面不许再往上盖（同 afterFrame 的抑制条件）。
    if (String(bootAttr('data-rustlua-boot') || '') === 'failed') return;
    var ctx = null;
    try { ctx = canvas.getContext('2d'); } catch (e) { ctx = null; }
    if (!ctx) return;
    if (packState === 'loading') {
      withAdapterDraw(ctx, drawLoadingBar);
      return;
    }
    if (packState === 'failed') {
      withAdapterDraw(ctx, packBootRetryNotice ? drawPackRetryNotice : drawPackFailure);
    }
  }

  function pumpPreBootFrame() {
    if (!preBootUi.active) return;
    preBootUi.ticks += 1;
    try {
      drawPreBootWait();
    } catch (error) {
      // 等待画面绝不能影响启动本身：画不出来时只留一句告警，继续把分包等到底。
      warn('wechat-adapter: boot 前等待画面绘制失败 -> ' + hostError(error));
    }
    preBootUi.id = setTimeout(pumpPreBootFrame, 16);
  }

  function startPreBootUi() {
    if (preBootUi.active) return;
    preBootUi.active = true;
    preBootUi.id = setTimeout(pumpPreBootFrame, 16);
  }

  function stopPreBootUi() {
    if (!preBootUi.active) return;
    preBootUi.active = false;
    try { clearTimeout(preBootUi.id); } catch (error) { /* 停不掉也不该影响启动：运行时随后自己接管画布 */ }
  }

  // ---------------------------------------------------------------- 输入桥接

  function toCanvasX(clientX) { return clientX / screenWidth * canvas.width; }
  function toCanvasY(clientY) { return clientY / screenHeight * canvas.height; }

  // canvas 像素 → **设计坐标**：与渲染端 contain 同一套算式（`VIEW_WIDTH/VIEW_HEIGHT` 是设计声明）。
  // 为什么不能直接把 canvas 像素当设计坐标交过去：虚拟摇杆/按钮的命中框是**设计空间**的矩形，
  // 差一个 contain 缩放就是"点哪儿都不对"，而这种错在读数上完全看不出来。
  var designFit = { w: 0, h: 0, scale: 1, ox: 0, oy: 0 };
  function designFromCanvas(px, py) {
    var cw = canvas.width || 1, ch = canvas.height || 1;
    if (designFit.w !== cw || designFit.h !== ch) {
      var scale = Math.min(cw / VIEW_WIDTH, ch / VIEW_HEIGHT);
      if (!(scale > 0)) scale = 1;
      designFit = {
        w: cw, h: ch, scale: scale,
        ox: (cw - VIEW_WIDTH * scale) / 2,
        oy: (ch - VIEW_HEIGHT * scale) / 2
      };
    }
    return [(px - designFit.ox) / designFit.scale, (py - designFit.oy) / designFit.scale];
  }

  // ── B4：把同一份事件也喂给**宿主自己的输入生产者** ────────────────────────────────
  //
  // 两条通道并存是刻意的：旧世界还在（删它是步骤 3），而宿主必须能在**旧运行时不跑**时自己
  // 收到输入——此前 `LATEST_FRAME` 的唯一写者是旧运行时，它一停，宿主每帧拿默认值：
  // 不崩溃、不报错、`presented` 照涨，只是人钉死（`tasks/plan/M73-01` 的 B4/R1）。
  //
  // 只在原子模式下发（B8 起这就是唯一形态；原"旧形态一行不动"的说法已过时）。
  // ── 多指：微信侧把**每根手指**喂给宿主的多指通路 ─────────────────────────────
  //
  // 为什么不复用 `feedHostInput`：它拿到的已经是"映射成 DOM 鼠标事件"的东西，**手指身份在那一步
  // 就丢了**（桥只维护一个跟手触点 `pointer`）。多指要的是 `touch.identifier` 本身。
  function feedHostTouch(kind, touch) {
    // （B8：原 `if (!rustSceneHost) return;` 已删 —— 原子宿主恒开。）
    var host = engineHostRef;
    if (!host || !touch) return;
    var fn = host["engine_host_input_pointer_" + kind];
    if (typeof fn !== "function") return;   // 旧内核没这三个导出：静默跳过（与既有降级一致）
    var design = designFromCanvas(Number(touch.clientX) || 0, Number(touch.clientY) || 0);
    var id = Number(touch.identifier);
    try {
      if (kind === "up") fn.call(host, id);
      else fn.call(host, id, design[0], design[1]);
    } catch (error) { return; }
    engineHost.hostTouchEvents += 1;
    // 同时按住的根数：这是微信侧"两指同时按"的可断言读数（与宿主侧的 `multipoint_frames` 同口径，
    // 但这一份是**适配层自己数的**，所以能证明"真手指到了导出"，而不只是"宿主报了数"）。
    if (kind === "down") engineHost.hostTouchActive[id] = true;
    else if (kind === "up") delete engineHost.hostTouchActive[id];
    if (Object.keys(engineHost.hostTouchActive).length > 1) engineHost.hostTouchMulti += 1;
  }

  function feedHostInput(event) {
    // （B8：原 `if (!rustSceneHost) return;` 已删 —— 原子宿主恒开。）
    var host = engineHostRef;
    if (!host || !event) return;
    var type = event.type;
    if (type === "keydown" || type === "keyup") {
      if (typeof host.engine_host_input_key !== "function") return;
      try {
        var accepted = host.engine_host_input_key(String(event.key == null ? "" : event.key), type === "keydown");
        if (accepted === false) engineHost.hostInputDroppedKeys += 1;
      } catch (error) { /* 单次事件失败不掀翻启动 */ }
      return;
    }
    if (type === "mousedown" || type === "mousemove" || type === "mouseup") {
      var design = designFromCanvas(Number(event.clientX) || 0, Number(event.clientY) || 0);
      if (typeof host.engine_host_input_pointer === "function") {
        try { host.engine_host_input_pointer(design[0], design[1]); } catch (error) { /* 同上 */ }
      }
      if ((type === "mousedown" || type === "mouseup") && typeof host.engine_host_input_button === "function") {
        try { host.engine_host_input_button("left", type === "mousedown"); } catch (error) { /* 同上 */ }
      }
    }
  }

  // 触摸 → 指针事件（让运行时既有的 mousedown/mousemove/mouseup 逻辑继续生效）
  // + 动作键（右半屏 space，菜单确认；兼容手机端从标题进游戏/对话推进）。
  //
  // 历史形态的"左半屏 WASD 虚拟摇杆"已移除：触屏操控改由游戏侧的 `ui/virtual_pad.lua`
  // （可见摇杆 + 可见按钮）接管，适配层只负责把触点忠实地翻译成鼠标事件——
  // 微信实测导致可见控件全部失效的三个根因都在这层：
  //   ① 光标只在 touchmove 时更新 → 首次按下（touchstart）时 cursor 还停在 (0,0)，
  //      虚拟控件在"按下边沿"那一帧全部命中测试脱靶；
  //   ② mouseup 只派发到 canvas，而运行时在 window 上监听（shim 的 dispatchEvent 不冒泡）
  //      → mouse_just_released 永远不触发，控件按下后永远不释放；
  //   ③ 只有首个触点派发 mousedown → 按住摇杆再点按钮时按钮收不到按下边沿。
  // 第③条的配套：运行时每次 mousedown 都记按下边沿 + 按下位置（press_cursor），
  // 虚拟按钮按按下位置命中测试（见 web-runtime.js 与 virtual_pad.lua）。



  function installTouchBridge() {
    var pointer = null;      // 驱动光标/持续鼠标状态的触点（优先左半屏=摇杆区触点）
    var actionTouch = null;  // 右半屏触点 → space（菜单确认）

    function pressKey(key) {
      dispatchWindow(makeEvent('keydown', { key: key, code: key }));
    }
    function releaseKey(key) {
      dispatchWindow(makeEvent('keyup', { key: key, code: key }));
    }

    function movePointer(touch, type, button) {
      dispatchCanvas(makeEvent(type, {
        button: button || 0,
        clientX: toCanvasX(touch.clientX || 0),
        clientY: toCanvasY(touch.clientY || 0)
      }));
    }

    function handleStart(res) {
      var list = (res && res.touches) || [];
      for (var i = 0; i < list.length; i++) {
        var touch = list[i];
        var isLeft = touch.clientX < screenWidth / 2;
        var becomesPointer = pointer === null || (isLeft && pointer.x >= screenWidth / 2);
        if (becomesPointer) {
          pointer = { id: touch.identifier, x: touch.clientX, y: touch.clientY };
          // 先落光标再按下去（根因①的修复）：虚拟控件在"按下边沿"那一帧用 cursor
          // 命中测试。光标随后由 pointer 触点的 touchmove 持续驱动。
          movePointer(touch, 'mousemove');
          movePointer(touch, 'mousedown', 0);
        } else {
          // 第二根及以后的手指（通常是在摇杆区外的按钮手）：只派发 mousedown、不移动
          // 光标——光标保持跟手（摇杆方向不被另一只手带偏），按下位置由运行时记进
          // press_cursor，虚拟按钮按它做命中测试（根因③的修复）。
          movePointer(touch, 'mousedown', 0);
        }
        feedHostTouch("down", touch);
        if (!isLeft && actionTouch === null) {
          actionTouch = touch.identifier;
          pressKey(' ');
        }
      }
    }
    function handleMove(res) {
      var list = (res && res.touches) || [];
      for (var i = 0; i < list.length; i++) {
        var touch = list[i];
        feedHostTouch("move", touch);
        if (pointer && touch.identifier === pointer.id) {
          pointer.x = touch.clientX;
          pointer.y = touch.clientY;
          movePointer(touch, 'mousemove');
        }
      }
    }
    function handleEnd(res) {
      var list = (res && res.changedTouches) || [];
      var remaining = (res && res.touches) || [];
      var lastUp = null;   // 最后一指（需要把 mouseup 交给运行时）
      for (var i = 0; i < list.length; i++) {
        var touch = list[i];
        feedHostTouch("up", touch);
        if (actionTouch !== null && touch.identifier === actionTouch) {
          releaseKey(' ');
          actionTouch = null;
        }
        if (pointer && touch.identifier === pointer.id) {
          pointer = null;
          // 还有触点时把光标交给下一个（优先左半屏，即摇杆区）。
          for (var j = 0; j < remaining.length; j++) {
            var t = remaining[j];
            if (t.identifier === touch.identifier) continue;
            if (pointer === null || (t.clientX < screenWidth / 2 && pointer.x >= screenWidth / 2)) {
              pointer = { id: t.identifier, x: t.clientX, y: t.clientY };
              movePointer(t, 'mousemove');
            }
          }
        }
        if (remaining.length === 0) lastUp = touch;
      }
      // 最后一指离屏才发 mouseup：按住摇杆时点按钮/确认不该把移动一起释放；
      // 无论最后抬起的是不是跟手触点，全部手指离屏都必须把"抬起"交给运行时
      // （否则控件按下后永远不释放）。同时派发到 window——运行时在 window 上监听
      // mouseup（根因②的修复）。
      if (lastUp) {
        movePointer(lastUp, 'mouseup', 0);
        dispatchWindow(makeEvent('mouseup', {
          button: 0,
          clientX: toCanvasX(lastUp.clientX || 0),
          clientY: toCanvasY(lastUp.clientY || 0)
        }));
      }
    }

    function on(name, handler) {
      if (typeof wx[name] === 'function') { try { wx[name](handler); } catch (err) { warn('wechat-adapter: ' + name + ' 注册失败 ' + err); } }
    }
    on('onTouchStart', handleStart);
    on('onTouchMove', handleMove);
    on('onTouchEnd', handleEnd);
    on('onTouchCancel', handleEnd);

    // 微信 PC 端小游戏支持键盘：直接桥接成运行时的 keydown/keyup。
    function normalizeKey(key) {
      var value = String(key == null ? '' : key);
      if (value === 'Spacebar' || value === ' ') return ' ';
      return value.length === 1 ? value.toLowerCase() : value;
    }
    on('onKeyDown', function (res) {
      dispatchWindow(makeEvent('keydown', { key: normalizeKey(res && res.key), code: res && res.code }));
    });
    on('onKeyUp', function (res) {
      dispatchWindow(makeEvent('keyup', { key: normalizeKey(res && res.key), code: res && res.code }));
    });
  }
  function dispatchWindow(event) { feedHostInput(event);
    // 派发目标是**运行时看到的那个 window**（宿主自带 window 时它是宿主对象，不是 globalThis）。
    var target = (typeof windowTarget !== 'undefined' && windowTarget) ? windowTarget : g;
    if (typeof target.dispatchEvent === 'function') target.dispatchEvent(event);
    else if (typeof g.dispatchEvent === 'function') g.dispatchEvent(event);
    var onname = 'on' + event.type;
    if (typeof target[onname] === 'function') { try { target[onname](event); } catch (e) { /* 忽略业务回调异常 */ } }
  }
  function dispatchCanvas(event) { feedHostInput(event);
    if (typeof canvasTarget.dispatchEvent === 'function') canvasTarget.dispatchEvent(event);
  }

  // ---------------------------------------------------------------- 生命周期与帧循环

  function installLifecycle() {
    function on(name, handler) {
      if (typeof wx[name] === 'function') { try { wx[name](handler); } catch (e) { warn('wechat-adapter: ' + name + ' 注册失败 ' + e); } }
    }
    // ④ 前后台要让**引擎宿主**也看得见：只派发 window blur/focus 是不够的——引擎订的是
    // document 的 visibilitychange（gpu_host.rs LifecycleEvents），而文档侧此前是 no-op。
    // 现在写 hidden 旗 + 在运行时的 document（可能是宿主那份，见 installShimGlobal）上派发；
    // blur/focus 原样保留（老 JS 世界读的是它们）。
    //
    // 实测（run19）：开发者工具的壳给的是一个**原生 DOM document**——它的 addEventListener
    // 因"够用"被 installShimGlobal 保留，而原生 dispatchEvent 用类型检查拒掉 plain 对象
    // （"is not of type 'Event'"）。所以派发要带两步兜底：plain 事件被拒 → 换真 Event 再派；
    // 并给 shim 注册表补派一次（宿主保留原生监听器时，注册在 shim 里的订阅者不能饿死）。
    function dispatchVisibility(hidden) {
      documentHidden = hidden;
      var target = (typeof documentTarget !== 'undefined' && documentTarget) ? documentTarget : documentShim;
      try {
        // 宿主 document 不可扩展时 getter 装不上（installShimGlobal 会 warn"补不动的 hidden"），
        // 这里逐次把当前值写过去，至少让 document.hidden 的读数跟着生命周期走。
        if (target !== documentShim) { try { target.hidden = hidden; } catch (e) { /* 属性写不进 */ } }
        if (typeof target.dispatchEvent === 'function') {
          var event = makeEvent('visibilitychange', {});
          try {
            target.dispatchEvent(event);
          } catch (hostEventError) {
            // 走到这里说明 target 是**真 DOM document**（run21 实锤：ctor=HTMLDocument）。
            // 造真 Event 的三路（run22 现场：createEvent 路线没能给出可派发的对象，原因待
            // 下面这条带形态读数的 warn 定案——读数缺 createEvent/defaultView/Event 身份三问）：
            //   1) `document.createEvent('Events') + initEvent`——DOM 自产，最正统；
            //   2) `document.defaultView.Event`——宿主壳里 document 与游戏全局**可能不同 realm**，
            //      全局 `Event` 可能是适配层占位类（defineGlobal 的 onlyIfMissing 遮法），
            //      而 defaultView 拿到的才是 document 那个 realm 的构造器；
            //   3) 全局 `Event`——前两条都没有才试它。
            var nativeEvent = null;
            var routes = 'createEvent=' + typeof target.createEvent +
              ' defaultView=' + (target.defaultView ? 'yes' : 'no') +
              ' viewEvent=' + (target.defaultView && target.defaultView.Event ? 'yes' : 'no');
            try {
              if (typeof target.createEvent === 'function') {
                nativeEvent = target.createEvent('Events');
                nativeEvent.initEvent('visibilitychange', false, false);
              } else if (target.defaultView && typeof target.defaultView.Event === 'function') {
                nativeEvent = new target.defaultView.Event('visibilitychange');
              } else if (typeof Event === 'function') {
                nativeEvent = new Event('visibilitychange');
              }
            } catch (e) { nativeEvent = null; }
            if (nativeEvent) {
              try { target.dispatchEvent(nativeEvent); }
              catch (e2) {
                warn('wechat-adapter: visibilitychange 真 Event 也派发失败 -> ' + hostError(e2) +
                  '（' + routes + ' eventCtor=' + (nativeEvent.constructor && nativeEvent.constructor.name) + '）');
              }
            } else {
              warn('wechat-adapter: visibilitychange 派发失败且造不出真 Event -> ' + hostError(hostEventError) +
                '（' + routes + '）');
            }
          }
        } else {
          warn('wechat-adapter: document 没有 dispatchEvent，visibilitychange 未派发（target=' +
            (target === documentShim ? 'shim' : '宿主') + '）');
        }
        // shim 注册表补派：target 用的不是 shim 那套分发、且双路包装**没装成**时才补
        // （包装在场时 shim 一路已由 wrapper 分发，补派会把同一注册表的订阅者打两遍）。
        if (target !== documentShim && target.dispatchEvent !== documentShim.dispatchEvent &&
          !visibilityChannelWrapped) {
          documentShim.dispatchEvent(makeEvent('visibilitychange', {}));
        }
      } catch (error) {
        warn('wechat-adapter: visibilitychange 派发失败 -> ' + hostError(error));
      }
    }
    on('onHide', function () { dispatchVisibility(true); dispatchWindow(makeEvent('blur', {})); });
    on('onShow', function () { dispatchVisibility(false); dispatchWindow(makeEvent('focus', {})); });
    on('onMemoryWarning', function (res) { warn('wechat-adapter: 内存告警 level=' + (res && res.level)); });
  }

  var nativeRaf = typeof wx.requestAnimationFrame === 'function' && typeof wx.cancelAnimationFrame === 'function';

  // ---------------------------------------------------------------- 字体

  // 字体取自 `pack/project.toml` 的 `font` 键：assetc 打包时已把完整字体子集化，
  // 子集文件就在 `pack/` 内，因此小游戏包不必再塞 17MB 的完整字体。
  // 把逻辑路径换成宿主**能直接吃**的本地路径：优先已落地的缓存文件，
  // 否则走 hostSrcFor（主体路由 + 已探明的方言）。
  function localPathFor(path) {
    var fs = fsManager();
    var cached = cachePathFor(path);
    if (cached && fs) {
      try { fs.readFileSync(cached); return cached; } catch (error) { /* 未缓存 */ }
    }
    return hostSrcFor(path);
  }

  // 确保某个资源在本地可用（包内/缓存都没有就尝试远端）。
  // 返回 Promise<{ available, source, localPath, subject }>：`source` 用来区分"能在代码包里直接读"
  // 还是"已经落到用户目录"——两者对 Image.src / wx.loadFont 这类**由宿主加载**的接口
  // 意义完全不同（见 patchImageSrc）；`localPath` 是**宿主可用的路径**（包内命中时就是那次
  // 读取真正成功的候选路径，缓存命中时是绝对缓存路径）。
  function ensureLocalAsset(path) {
    var existing = readWithStrategies(path, readBytesRaw);
    if (existing) {
      noteRead(path, existing);
      return Promise.resolve({
        available: true,
        source: existing.source,
        localPath: existing.source === 'cache' ? localPathFor(path) : existing.candidate,
        subject: existing.subject
      });
    }
    if (!loadRemoteConfig()) return Promise.resolve({ available: false, source: null, localPath: null, subject: null });
    return fetchRemote(path).then(function (ok) {
      return { available: ok, source: ok ? 'cache' : null, localPath: ok ? cachePathFor(path) : null, subject: null };
    });
  }

  function loadGameFont() {
    var projectPath = PACK_PREFIX + 'project.toml';
    // project.toml 可能只在远端（资源移出包内时）——先确保它在本地，否则会静默跳过字体。
    return ensureLocalAsset(projectPath).then(function () {
      var projectText = readText(projectPath);
      var relative = '';
      if (projectText) {
        var lines = projectText.split(/\r?\n/);
        for (var i = 0; i < lines.length; i++) {
          var match = /^\s*font\s*=\s*"(.+)"\s*$/.exec(lines[i]);
          if (match) { relative = match[1]; break; }
        }
      }
      if (!relative) { warn('wechat-adapter: pack/project.toml 未声明 font，跳过自定义字体'); return null; }
      var fontPath = PACK_PREFIX + relative.replace(/^\.?\//, '');
      // 字体注册由适配层负责（wx.loadFont + wrapContext 里的 ctx.font 族名替换）。留个标记给
      // web-runtime.js：它那条浏览器路（FontFace + document.fonts.add）在小游戏里必然撞上宿主
      // 自己的 FontFaceSet，抛 "parameter 1 is not of type 'FontFace'"。
      setWindowFlag('__RUSTLUA_FONT_OWNER', 'wechat-adapter');
      setWindowFlag('__RUSTLUA_FONT', fontPath);
      return ensureLocalAsset(fontPath).then(function (result) {
        if (!result.available) { warn('wechat-adapter: 字体资源不可得，中文将使用系统字体: ' + fontPath); return null; }
        // 优先用**那次读取真正成功**的宿主路径（分包形态下它已经被路由到 mainpack/）；
        // 只有拿不到时才回落到按方言重算（缓存命中与远端下载后是这种情况）。
        var localPath = result.localPath || localPathFor(fontPath);
        var family = safeCall(wx, 'loadFont', localPath);
        if (!family && typeof canvas.loadFont === 'function') family = safeCall(canvas, 'loadFont', localPath);
        if (!family) { warn('wechat-adapter: wx.loadFont 不可用，中文将使用系统字体: ' + localPath); return null; }
        gameFontFamily = family;
        log('wechat-adapter: 已加载游戏字体 ' + localPath + ' → ' + family);
        return family;
      });
    });
  }

  // ---------------------------------------------------------------- 安装全局宿主

  // 宿主（Node 的无头验证器 / 浏览器）可能已把 window、navigator、performance 定义为
  // 只读访问器，直接赋值会抛 TypeError。这里统一走"能写就写、不能写就 defineProperty"。
  function defineGlobal(name, value, onlyIfMissing) {
    if (onlyIfMissing && typeof g[name] !== 'undefined') return false;
    try {
      g[name] = value;
      return true;
    } catch (e) { /* 落到 defineProperty */ }
    try {
      Object.defineProperty(g, name, { value: value, writable: true, configurable: true });
      return true;
    } catch (e) {
      warn('wechat-adapter: 无法定义全局 ' + name + '（' + e + '）');
      return false;
    }
  }

  // 成员"够用"的判定：函数必须可调用；body/documentElement/fonts 这类对象必须至少带一个
  // 我们真正会调用的成员（setAttribute / add / appendChild / addEventListener），
  // 否则宿主给的半成品和"没有"等价；字符串等标量（readyState）视为可用。
  function memberUsable(value) {
    if (value === undefined || value === null) return false;
    if (typeof value === 'function') return true;
    if (typeof value === 'object') {
      return (typeof value.setAttribute === 'function') || (typeof value.add === 'function') ||
        (typeof value.appendChild === 'function') || (typeof value.addEventListener === 'function');
    }
    return true;
  }

  // 个别成员的"够用"标准要更严：`body`/`documentElement` 必须能 `setAttribute`
  // （web-runtime 的启动/画面标记全靠它写出来），`fonts` 必须有 `add`。
  // 放宽的后果很隐蔽：宿主那个半成品 body 会让 `data-rustlua-boot`/`data-rustlua-drawn` 全都写不进去，
  // 于是**诊断自己开始撒谎**（画面明明画了，我们却报 drawn=0）。
  var SHIM_MEMBER_REQUIRED = { body: 'setAttribute', documentElement: 'setAttribute', fonts: 'add' };
  function shimMemberUsable(key, value) {
    var required = SHIM_MEMBER_REQUIRED[key];
    if (required) return !!(value && typeof value[required] === 'function');
    return memberUsable(value);
  }

  function tryPatchMember(target, key, value) {
    try {
      target[key] = value;
      if (target[key] === value) return true;
    } catch (e) { /* 落到 defineProperty */ }
    try {
      Object.defineProperty(target, key, { value: value, writable: true, configurable: true });
      return target[key] === value;
    } catch (e) { return false; }
  }

  // 每个 shim 全局的安装结果（接替/补齐/补不动/仍缺哪些），进 __RUSTLUA_DIAG() 供现场核对。
  var shimReports = {};

  // 把一个浏览器风格的 shim 装成全局，并且**回读校验**。
  // 为什么必须回读：非严格模式下对不可写属性赋值是**静默失败**的，而微信开发者工具的壳里
  // 自带半套 DOM（`document` 已被占用：实测 `Object.keys(document) === ['location']`，且
  // **不可扩展**——只有已存在的自有属性还能重定义，`getElementById` 就是被这样补上的）。
  // 早期实现见 `defineGlobal` 返回 true 就以为装上了，宿主那个残缺对象继续留在全局，
  // 直到运行时调 `document.getElementById` 才炸出难以定位的 TypeError。
  //
  // 语义：能接替就接替；接替不了就把 shim 的成员**就地补齐**到宿主对象上；
  // 补不齐的成员（对象不可扩展/属性不可配置）**绝不在这里抛**——模块加载期抛出去会让
  // `game.js` 整个中止、`__RustLuaWechat` 都不存在，屏幕上只剩一片黑加一行控制台错误
  // （实测过）。改成：console.error 喊出来 + 记进 `__RUSTLUA_DIAG()`，真正的失败交给 `start()`
  // 的错误路径——那条路径会把原因**画到屏幕上**，比"什么都没启动"好诊断得多。
  function installShimGlobal(name, shim, required) {
    var host = g[name];
    var replaced = false;
    if (host === shim) {
      replaced = true;
    }
    else {
      try { g[name] = shim; } catch (e) { /* 落到 defineProperty */ }
      if (g[name] !== shim) {
        try { Object.defineProperty(g, name, { value: shim, writable: true, configurable: true }); } catch (e) { /* 宿主占着，下面就地补齐 */ }
      }
      replaced = (g[name] === shim);
    }
    var target = replaced ? shim : g[name];
    var patched = [];
    var blocked = [];
    if (!replaced) {
      var keys = Object.keys(shim);
      for (var i = 0; i < keys.length; i++) {
        var key = keys[i];
        if (shimMemberUsable(key, target[key])) continue;
        if (tryPatchMember(target, key, shim[key])) patched.push(key);
        else blocked.push(key);
      }
      var hostKeys = (target && typeof target === 'object') ? Object.keys(target) : [];
      warn('wechat-adapter: 宿主已占用全局 ' + name + '（宿主 keys: ' + (hostKeys.length ? hostKeys.join(',') : '（空对象）') + '）' +
        (patched.length ? '，已就地补齐 ' + patched.join(',') : '') +
        (blocked.length ? '，补不动的 ' + blocked.join(',') : ''));
    }
    var missing = [];
    for (var j = 0; j < required.length; j++) {
      if (!shimMemberUsable(required[j], target[required[j]])) missing.push(required[j]);
    }
    shimReports[name] = { replaced: replaced, patched: patched, blocked: blocked, missing: missing, hostKeys: (target && typeof target === 'object') ? Object.keys(target) : [] };
    if (missing.length) {
      var ownKeys = shimReports[name].hostKeys;
      console.error('wechat-adapter: 全局 ' + name + ' 被宿主占用且补不齐（缺 ' + missing.join(',') +
        '；宿主现有 keys: ' + (ownKeys.length ? ownKeys.join(',') : '（无）') + '；补不动的成员: ' + (blocked.length ? blocked.join(',') : '（无）') +
        '）。运行时一旦用到这些成员就会抛 TypeError —— 那时错误会连同本行一起出现在控制台，' +
        '并且原因会被画到屏幕上（不是静默黑屏）。');
    }
    return target;
  }

  // `window` 必须带事件通道（运行时把 rustluaRunLuaGame 挂在它上面，键盘/鼠标事件也从
  // 这里派发）。通道先装在 globalThis 上，再让 installShimGlobal 去接替/补齐宿主 window——
  // 顺序不能反：installShimGlobal 会校验 addEventListener 是否真的可用。
  createEventTarget(g);
  var windowTarget = installShimGlobal('window', g, ['addEventListener', 'removeEventListener', 'dispatchEvent']);
  if (windowTarget !== g) {
    // 宿主 window 与 globalThis 不是同一个对象：事件通道要落在 window 上
    // （game.js / web-runtime.js 读的都是 bare `window`）。
    createEventTarget(windowTarget);
  }
  // 早期 fengari-web（已随 WP10 迁移从发布包移除）的 UMD 包在加载时会做
  // `document instanceof HTMLDocument` 这类浏览器探测：小游戏（以及无头验证器）没有这些
  // DOM 类，缺少时 ReferenceError 会直接打断加载。这里提供"什么都匹配不上"的占位类，
  // 让探测退化为 false，而不是抛错（对 wasm 桥无影响，保留以防宿主其它依赖同样探测）。
  defineGlobal('HTMLDocument', function HTMLDocument() {}, true);
  defineGlobal('HTMLElement', function HTMLElement() {}, true);
  defineGlobal('HTMLCanvasElement', function HTMLCanvasElement() {}, true);
  defineGlobal('Element', function Element() {}, true);
  defineGlobal('Node', function Node() {}, true);
  defineGlobal('Event', function Event() {}, true);
  defineGlobal('XMLHttpRequest', function XMLHttpRequest() {}, true);
  defineGlobal('fetch', fetchShim, false);
  // document：微信开发者工具的壳里**已经有一个不可写的 document**（实测 `Object.keys` 只有
  // `location`、且对象不可扩展：只有已存在的自有属性还能重定义，`getElementById` 正是被这样
  // 补上的），这就是 "document.getElementById is not a function" 的来源；这里走"接替或补齐"。
  // 硬要求只有 `getElementById`——web-runtime.js 里唯一**没有前置保护**的 DOM 依赖就是它
  // （`body`/`documentElement` 有 `if (!body) return`，`fonts` 有 `!document.fonts` 判断）。
  var documentTarget = installShimGlobal('document', documentShim, ['getElementById']);
  // ④ 生命周期通道保底（run21/22 实锤的形态）：开发者工具的壳给的是一个**仿冒 HTMLDocument
  // 的 stub**——原生 addEventListener 因"够用"被 installShimGlobal 保留，但它的 dispatchEvent
  // 只收真 DOM Event（`makeEvent` 的 plain 对象被类型检查拒掉；stub 又没有可用的
  // createEvent/defaultView 来造真 Event）。结果就是"订阅成功、事件永远到不了"。
  // 这里把 documentTarget 的 add/remove/dispatchEvent 换成**双通道分发**：注册同时进
  // shim 注册表（我们控制的那套，plain 事件也认）与原生监听器（若可用）；派发先走 shim
  // 注册表、再尽力转原生（失败吞掉——shim 那一步已经保证送达）。引擎的
  // `LifecycleEvents` 与探针因此都收得到。
  var visibilityChannelWrapped = false;
  (function ensureVisibilityChannel() {
    if (documentTarget === documentShim) return; // shim 自己就是目标，无需再包
    var nativeAdd = typeof documentTarget.addEventListener === 'function' ? documentTarget.addEventListener.bind(documentTarget) : null;
    var nativeRemove = typeof documentTarget.removeEventListener === 'function' ? documentTarget.removeEventListener.bind(documentTarget) : null;
    var nativeDispatch = typeof documentTarget.dispatchEvent === 'function' ? documentTarget.dispatchEvent.bind(documentTarget) : null;
    try {
      documentTarget.addEventListener = function (type, handler) {
        documentShim.addEventListener(type, handler);
        if (nativeAdd) { try { nativeAdd(type, handler); } catch (e) { /* 原生不收的形态由 shim 兜 */ } }
      };
      documentTarget.removeEventListener = function (type, handler) {
        documentShim.removeEventListener(type, handler);
        if (nativeRemove) { try { nativeRemove(type, handler); } catch (e) { /* 同上 */ } }
      };
      documentTarget.dispatchEvent = function (event) {
        documentShim.dispatchEvent(event);
        if (nativeDispatch) { try { nativeDispatch(event); } catch (e) { /* 原生要真 Event，够不到就算 */ } }
        return true;
      };
      visibilityChannelWrapped = true;
      log('wechat-adapter: document 事件通道已改双路分发（shim 注册表 + 宿主原生尽力转发）');
    } catch (error) {
      warn('wechat-adapter: document 双路分发包装失败（visibilitychange 只剩 shim 一路）-> ' + hostError(error));
    }
  })();
  log('wechat-adapter: 宿主全局自检 window=' + (windowTarget === g ? 'globalThis' : '宿主 window（已补齐）') +
    ' document=' + (documentTarget === documentShim ? '适配层 shim' : '宿主 document（已补齐）') +
    ' canvas=' + (canvasTarget === canvas ? '原对象' : '转发包装') +
    ' getElementById=' + (typeof documentTarget.getElementById === 'function' ? 'ok' : 'missing') +
    ' body=' + (typeof documentTarget.body === 'object' && documentTarget.body ? 'ok' : 'missing'));
  defineGlobal('navigator', { userAgent: 'wechat-minigame', platform: (sysInfo.platform || 'wechat') }, true);
  defineGlobal('performance', { now: function () { return Date.now(); } }, true);
  // 运行时（web-runtime.js）读的是 bare `window`，因此给运行时的标记必须写到**运行时看到的
  // 那个 window** 上；宿主自带 window 时那不是 globalThis（两处都写，谁读都能读到）。
  function setWindowFlag(name, value) {
    var target = (typeof windowTarget !== 'undefined' && windowTarget) ? windowTarget : g;
    var written = false;
    try {
      target[name] = value;
      written = (target[name] === value);
    } catch (e) { /* 落到 defineProperty */ }
    if (!written) {
      try { Object.defineProperty(target, name, { value: value, writable: true, configurable: true }); written = true; } catch (e) { /* 下面兜底 */ }
    }
    if (!written) warn('wechat-adapter: 无法写入 window.' + name + '（宿主 window 只读）');
    if (target !== g) { try { g[name] = value; } catch (e) { /* 可忽略：运行时读 window */ } }
  }

  // 逻辑路径 → 交给**宿主自己加载**的接口（Image.src / wx.loadFont）的路径。
  // 两件事必须依次做，缺一不可：
  //   ① 主体路由：分包形态下首屏文件在 mainpack/，非首屏在 pack/。多根布局下"只换方言"
  //      是不够的——`pack/a.png` 与 `mainpack/a.png` 是两个不同的文件（构建期是搬运不是复制，
  //      两侧互斥），只换方言会把路径指到那份**不存在的**文件上，症状是"文本读得到、图片全黑"。
  //   ② 方言：宿主对代码包路径的接受规则是同一套（文本读得到、图片却 404 就是这里没跟上）。
  // 做过的映射记在 hostSrcMemo 里：Image.src 的 setter 每张图都会走一次，没必要重复算。
  var hostSrcMemo = Object.create(null);
  function hostSrcFor(path) {
    var normalized = normalizePath(path);
    var memoized = hostSrcMemo[normalized];
    if (memoized) return memoized;
    var subject = readSubjects(normalized)[0];
    var remembered = preferredStrategy[strategyKeyOf(subject)];
    // 方言还没探明时**不记账**：那时的结果只是"主体原样"，等方言学出来之后再记，
    // 否则会把一个没有方言前缀的路径永远钉在 memo 里（症状是"后来探明了方言，贴图却还读不到"）。
    if (!remembered) return subject;
    var mapped = subject;
    for (var i = 0; i < readStrategies.length; i++) {
      if (readStrategies[i].name === remembered) {
        var candidate = readStrategies[i].apply(subject);
        if (candidate) mapped = candidate;
        break;
      }
    }
    hostSrcMemo[normalized] = mapped;
    return mapped;
  }

  // 逻辑资源 → 交给**宿主自己加载**的接口（Image.src / InnerAudioContext.src）的路径。
  //
  // 与 patchImageSrc 的三步同源（主体路由 → 分包挂载 → 远端落缓存），但这里是**同步**的：
  // 音频的 play() 是同步 API（Lua 侧 audio.play 要一个即时结果），没法像贴图那样等 Promise
  // 再改 src。所以判据换成"现在够不够得着"：
  //   * 用户目录缓存命中 → 用绝对缓存路径（远端形态下包内确实没有这个文件）；
  //   * 包内命中（mainpack/ 或 pack/，含已探明的方言）→ 用那个宿主路径；
  //   * 包内没有但配了远端 → **直接把远端 URL 交给宿主**：InnerAudioContext 支持网络资源，
  //     不必先下载落盘（贴图那条必须先下载，是因为宿主自己加载时没有"远端回落到缓存"的能力）。
  //     口径与远端形态既有前提一致：那条路本来就要配 downloadFile 域名白名单。
  //   * 分包还没挂载 → 报 `pending`：这不是"文件不存在"，只是"现在够不着"，调用方**不得**
  //     据此把它记成永久缺失（记了之后挂载完成这个音效整局都不再尝试）。
  // 探测一律用 accessSync（只问存在性、不读内容）：音频动辄上兆，用"读一遍字节"当存在性
  // 判据会让每次播放都白读一遍。
  function probeHostFile(path) {
    var fs = fsManager();
    // 探测不了就**不下结论**（返回 null，调用方按"假定存在"处理）：宁可让宿主去报真实的
    // 加载错误，也不要因为探针缺席而把一份本来好好的资源判成不存在。
    if (!fs || typeof fs.accessSync !== 'function') return null;
    try { fs.accessSync(path); return true; } catch (error) { return false; }
  }

  function assetSrcFor(path) {
    var normalized = normalizePath(path);
    var cached = cachePathFor(normalized);
    if (cached && probeHostFile(cached) === true) {
      return { src: cached, source: 'cache', pending: false };
    }
    var subject = hostSrcFor(normalized);
    if (probeHostFile(subject) !== false) {
      return { src: subject, source: 'package', pending: false };
    }
    if (needsPackMount(normalized)) {
      return { src: subject, source: 'pending', pending: true };
    }
    if (loadRemoteConfig()) {
      return { src: remoteUrlFor(normalized), source: 'remote', pending: false };
    }
    // 包内没有、远端也没配：把**正确的包内路径**交出去，让宿主按真实情况报错
    // （而不是我们静默吞掉，也不是交一个历史上猜出来的错误路径）。
    return { src: subject, source: 'missing', pending: false };
  }

  // Image.src 是**宿主自己加载**的接口：我们只能给它一个宿主能访问的路径，
  // 而且无法像 fetch 那样"读不到就回落到远端"。
  // 资源移出包内（瘦首包/远端回落）时，直接把 `pack/x.png` 交给宿主只会静默 onerror
  // ——贴图一张都出不来，而且在我们的读取统计里看不出来（实测无头 harness 曾因为
  // 直接从磁盘读文件而"全绿"，掩盖了这个真实缺口）。
  // 所以这里：包内能读到就原样交给宿主；读不到就先下载到 USER_DATA_PATH，再把 src
  // 指向那个本地文件。
  // 宿主的 `src` 到底定义在哪一层？三种形态都真实存在：
  //   ① own accessor —— 真机的 JS 层对象，以及本机无头 harness；
  //   ② **原型上的 class accessor** —— 微信基础库的 `class Image { setSrc(v){…} getSrc(){…} }`，
  //      对外的 `src` 是原型访问器，`Object.getOwnPropertyDescriptor(image,'src')` 拿到
  //      `undefined`（基础库 2.25.3 实测，见 README「Image.src 接管」）；
  //   ③ 包装/代理对象（描述符取不到），但暴露了 `setSrc` 方法。
  // 旧实现在拿不到 own 描述符时退化成 `image._src = value`，注释里写着"原始 setter"——
  // 那其实**什么都没做**：基础库是在 `setSrc` 里发起加载的，直接写 `_src` 不会触发任何加载，
  // 而运行时的 `loadImage` 只认 `onload`/`onerror`，于是那个 Promise 永不 settle →
  // `loadSceneImages` 卡死 → 屏幕全黑、0 error、0 帧，日志里却一条异常都没有（实测踩到）。
  // 所以这里沿原型链找**真能用的写入路径**；一个都找不到就返回 null，绝不假装成功。
  function makeAccessorWriter(descriptor, image, isOwn) {
    return {
      via: isOwn ? 'own-accessor' : 'prototype-accessor',
      write: function (value) { descriptor.set.call(image, value); }
    };
  }

  function resolveHostSrcWriter(image) {
    var level = image;
    while (level) {
      var descriptor = null;
      try { descriptor = Object.getOwnPropertyDescriptor(level, 'src'); } catch (error) { descriptor = null; }
      if (descriptor && typeof descriptor.set === 'function') {
        return makeAccessorWriter(descriptor, image, level === image);
      }
      var parent = null;
      try { parent = Object.getPrototypeOf(level); } catch (error) { parent = null; }
      if (!parent || parent === level) break;
      level = parent;
    }
    // 代理/包装对象取不到描述符，但基础库的 Image 类自带 setSrc（写它的效果 = 给它 src）。
    if (typeof image.setSrc === 'function') {
      return { via: 'setSrc-method', write: function (value) { image.setSrc(value); } };
    }
    return null;
  }

  // 宿主的 onload/onerror 也可能是**访问器**（基础库 2.32.3 的 `k$` 包装类就是：
  // `set onload(v){ Object.defineProperty(this,'__onload',{value:v}) }`，而真实 image 的
  // `onload` 是在**构造时**接成 `this.__onload` 的）。这种情况必须走它的 setter：
  // 用 own accessor 盖住 = 宿主的 setter 永不执行、`__onload` 永远 undefined，
  // 于是那座桥指向空气——贴图即使加载成功，回调也传不回运行时 → `loadImage` 的 Promise
  // 永不 settle → 启动卡死、0 错误、屏幕全黑（实测，与 src 是原型访问器同源的第二个缺口）。
  function resolveHostCallbackWriter(image, name) {
    var level = image;
    while (level) {
      var descriptor = null;
      try { descriptor = Object.getOwnPropertyDescriptor(level, name); } catch (error) { descriptor = null; }
      if (descriptor && typeof descriptor.set === 'function') {
        return function (value) { descriptor.set.call(image, value); };
      }
      var parent = null;
      try { parent = Object.getPrototypeOf(level); } catch (error) { parent = null; }
      if (!parent || parent === level) break;
      level = parent;
    }
    return null;
  }

  // 贴图**真的**加载成功与否只有宿主说了算：包一层 onload/onerror 计数，让"素材静默丢失"
  // 变成屏幕上的红色警示条（不依赖我们自己的推断——推断会漏掉宿主自己加载失败的路径）。
  function installImageCounters(image) {
    ['onload', 'onerror'].forEach(function (name) {
      var isError = (name === 'onerror');
      var handler = null;
      // 宿主有 setter 就把包装后的回调**转发**给它（不能只留在我们自己的字段里）；
      // 宿主只有普通数据属性才用我们自己的 accessor 独占（原来的行为，真机/无头 harness）。
      var hostSet = resolveHostCallbackWriter(image, name);
      var wrap = function (next) {
        if (typeof next !== 'function') return next;
        return function (event) {
          if (isError) {
            assetStats.imageFailed += 1;
            // 宿主**自己**说加载失败：这是最硬的证据（不是我们的推断）。
            // 把路径与"是不是我们自己给的 USER_DATA_PATH 缓存路径"记下来——
            // 真机上 `Image.src` 读不到缓存文件时，画面会整片缺素材却毫无提示。
            var source = String((image && image.src) || image._src || '');
            var fromCache = !!(userRoot && source.indexOf(userRoot) === 0);
            if (fromCache) assetStats.imageCacheLoadFailed += 1;
            if (assetStats.imageFailureSamples.length < 4) {
              assetStats.imageFailureSamples.push(source + (fromCache ? '（来自 USER_DATA_PATH 缓存）' : ''));
              warn('wechat-adapter: 贴图加载失败 ' + source + (fromCache ? '（来自 USER_DATA_PATH 缓存，宿主读不了这个路径？）' : ''));
            }
          }
          else assetStats.imageLoaded += 1;
          return next.call(image, event);
        };
      };
      if (hostSet && !callbackTakeover) {
        callbackTakeover = 'host-setter';
        log('wechat-adapter: Image.onload/onerror 接管方式 = host-setter（宿主用私有字段桥接回调）');
      }
      else if (!hostSet && !callbackTakeover) {
        callbackTakeover = 'own-accessor';
        log('wechat-adapter: Image.onload/onerror 接管方式 = own-accessor');
      }
      try {
        Object.defineProperty(image, name, {
          configurable: true,
          enumerable: true,
          get: function () { return handler; },
          set: function (next) {
            handler = wrap(next);
            // **关键**：写进我们自己的字段还不够，必须让宿主的 setter 也收到——
            // 宿主的回调桥读的是它自己的私有字段（`__onload`），不是 `image.onload`。
            if (hostSet) hostSet(handler);
          }
        });
      } catch (error) {
        // 宿主不让改写：只影响自证计数，不影响贴图本身，降级为告警。
        warn('wechat-adapter: 无法统计 Image.' + name + ' -> ' + hostError(error));
      }
    });
    return image;
  }

  // 有些发布形态**必须**能改写 Image.src，否则贴图必然静默全丢：
  //   remote 形态：重资源不在包内，不改写就无法把 src 指到下载后的本地缓存路径上；
  //   分包形态：首屏贴图被构建期搬到 mainpack/，而运行时要的是 `pack/…`（web-runtime.js 里
  //             硬编码了 pack/ 前缀），不改写就一张也对不上——不是"少几张"，是整批失败。
  // 这两种情况必须**响亮失败**：静默降级的结果是"画面全黑 + 日志里一条异常都没有"。
  function requiresSrcRewrite() {
    if (loadRemoteConfig()) return true;
    // 分包形态 ⇔ 首屏清单存在（见 packSubpackageName）：首屏贴图被切到 mainpack/，
    // 运行时要的是 pack/ 前缀，不改写 src 就整批对不上。
    return hasPackSubpackage();
  }

  // 原因分形态写清楚：两种形态的出路完全不同（remote 去掉配置即可，分包形态只能换发布形态）。
  // 措辞里保留"不允许改写 Image.src"这一串：harness 的冻结用例正是靠它判定"响亮"。
  function srcRewriteErrorMessage(prefix) {
    var tail = loadRemoteConfig()
      ? '远端回落的贴图会静默丢失并卡死启动；请确认基础库版本，或去掉 remote-assets.json 改为包内完整资源'
      : '但本包是**分包形态**：首屏贴图被切到 mainpack/ 下，而运行时要的是 pack/ 前缀，' +
        '不改写 src 就会整批丢图；请确认基础库版本，或改用 -AssetPack full / remote 发布';
    return 'wechat-adapter: ' + prefix + '，' + tail;
  }

  // 需要改写 src 却改不动的**收尾**：光抛不够。这个 throw 落进运行时 `new Image()` 的
  // **Promise 执行器**里，异常被吞掉后屏幕上只是"少一批贴图"，而 `data-rustlua-boot` 仍是 running
  // ——失败看起来像成功（实测：harness 的冻结用例正是这样判它"静默丢图"）。
  // 所以走与启动失败同一套证据：console.error（harness 与真机日志面板都认这一档）
  // + `data-rustlua-boot=failed`（诊断/回归门读的就是它）+ 错误页 + 像素探针。
  // **写标记必须写宿主那个元素**：`bootAttr()` 的取值顺序是"宿主 body/documentElement → 内部 stub"，
  // 只写 stub 会被运行时的 running 盖掉（实测：写完仍读到 boot=running）。
  var srcRewriteFatalMessage = '';

  function writeBootMark(name, value) {
    var targets = [];
    try {
      var host = documentTarget && (documentTarget.body || documentTarget.documentElement);
      if (host && typeof host.setAttribute === 'function') targets.push(host);
    } catch (e) { /* 宿主 document 形态不可预期 */ }
    if (bodyElement && typeof bodyElement.setAttribute === 'function') targets.push(bodyElement);
    for (var i = 0; i < targets.length; i++) {
      try { targets[i].setAttribute(name, String(value)); } catch (error) { /* 逐个尝试，写不进去就算了 */ }
    }
  }

  function restampSrcRewriteFatal() {
    if (!srcRewriteFatalMessage) return;
    try {
      writeBootMark('data-rustlua-boot', 'failed');
      writeBootMark('data-rustlua-boot-detail', srcRewriteFatalMessage.slice(0, 400));
    } catch (error) { /* 重申标记失败绝不能影响帧循环 */ }
  }

  function markSrcRewriteFatal(message) {
    logError(message);
    srcRewriteFatalMessage = String(message);
    restampSrcRewriteFatal();
    try {
      showBootError(message);
      drawStartupProbe();
    } catch (overlayError) {
      // 覆盖层/错误页失败绝不能再抛一次：那会把真正的失败原因顶掉。
    }
  }

  function patchImageSrc(image) {
    if (!image) return image;
    // 计数与 src 接管互不依赖：先装计数，任何一条路径都不会漏掉"贴图到底成没成"。
    installImageCounters(image);
    var writer = resolveHostSrcWriter(image);
    if (!writer) {
      // 接管不了的两种后果完全不同：包内完整的包照常能加载（宿主自己读 pack/），
      // 需要改写 src 的形态则必然静默丢图甚至卡在启动上——那种情况必须响亮失败（README §已知边界）。
      if (requiresSrcRewrite()) {
        // 消息里必须带"不允许改写 Image.src"这一串（harness 的冻结用例靠它判定"响亮"）。
        var noWriterMessage = srcRewriteErrorMessage('不允许改写 Image.src：宿主既没有可写的 src 访问器，也没有 setSrc');
        markSrcRewriteFatal(noWriterMessage);
        throw new Error(noWriterMessage);
      }
      warn('wechat-adapter: 无法接管 Image.src（宿主既没有可写的 src 访问器，也没有 setSrc），按原样交给宿主');
      return image;
    }
    if (!srcTakeover) {
      srcTakeover = writer.via;
      log('wechat-adapter: Image.src 接管方式 = ' + writer.via);
    }
    try {
      var hostSet = writer.write;
      var current = image.src;
      Object.defineProperty(image, 'src', {
        configurable: true,
        enumerable: true,
        get: function () { return current; },
        set: function (value) {
          var requested = String(value);
          var normalized = normalizePath(requested);
          var mapped = hostSrcFor(requested);
          current = mapped;
          // 未启用远端回落：保持原有同步行为（零额外开销、零行为变化）。
          if (scopeOf(requested) !== 'pack' || !loadRemoteConfig()) {
            // 分包形态下**非首屏**贴图分布在分包里，挂载前交给宿主必然立刻 onerror
            // （运行时的 loadImage 以第一次 onerror 为准，之后补挂载也来不及）。
            // 所以先等分包结算（成功或失败都一定会结算，见 ensurePack），再决定交给宿主什么：
            // 成功 → 正确的 pack/ 路径；失败 → 原样交出去，让宿主按真实情况报错（响亮，不静默）。
            if (needsPackMount(requested)) {
              var deferred = current;
              ensurePack().then(function () { hostSet(hostSrcFor(requested)); })
                .catch(function () { hostSet(deferred); });
              return;
            }
            hostSet(mapped);
            return;
          }
          // 启用远端时先探明"包内到底有没有"：**绝不能**先把一个不存在的路径交给宿主——
          // 宿主会立刻 onerror（运行时的 loadImage 以第一次 onerror 为准），而下载是异步的，
          // 等我们下载完再改 src 已经晚了（实测 28 张贴图就是这样丢的）。
          var existing = readWithStrategies(requested, readBytesRaw);
          if (existing) {
            noteRead(requested, existing);
            // 命中的可能是**已下载的缓存文件**（user-data）：那种情况下必须把绝对缓存路径
            // 交给宿主，交给它 `pack/x.png` 一样读不到（包内确实没有这个文件）。
            hostSet(existing.source === 'cache' ? localPathFor(requested) : existing.candidate);
            return;
          }
          ensureLocalAsset(requested).then(function (result) {
            if (result.available && result.localPath) {
              // 远端下载成功：本地缓存路径（或包内命中的候选路径）才是宿主读得到的那个。
              hostSet(result.localPath);
              log('wechat-adapter: 贴图改走本地缓存 ' + requested + ' -> ' + result.localPath);
            }
            else {
              // 远端也没有：把包内路径交给宿主，让它按真实情况报错（而不是我们静默吞掉）。
              hostSet(mapped);
              // 明知包内没有、远端也拿不到：这张贴图必然加载失败 → 计入"未供应"，
              // 屏幕上会出现红色警示条（否则就是"少了一半素材但画面全黑，日志里什么都没有"）。
              assetStats.imageMissing += 1;
              warn('wechat-adapter: 贴图不可得（包内与远端都没有） ' + requested);
            }
          }).catch(function (error) {
            hostSet(mapped);
            assetStats.imageMissing += 1;
            warn('wechat-adapter: 贴图回落失败 ' + requested + ' -> ' + hostError(error));
          });
        }
      });
    } catch (error) {
      // 宿主不允许我们改写 Image.src：**需要改写 src 的形态下这必须是响亮失败**。
      // 否则包内没有的贴图会一张张静默 onerror 消失——画面上少东西，日志里却什么都看不到
      // （本机的无头 harness 就因为"直接从磁盘读文件"而长期把这种缺失测成绿色）。
      // 包内完整的包（full 形态）不依赖改写，只告警即可，不能因此让游戏起不来。
      if (requiresSrcRewrite()) {
        var srcBlockedMessage = srcRewriteErrorMessage('宿主不允许改写 Image.src（' + hostError(error) + '）');
        markSrcRewriteFatal(srcBlockedMessage);
        throw new Error(srcBlockedMessage);
      }
      warn('wechat-adapter: 无法改写 Image.src 的路径形式 -> ' + hostError(error));
    }
    return image;
  }

  defineGlobal('Image', function Image() { return patchImageSrc(safeCall(wx, 'createImage')); }, false);
  defineGlobal('FontFace', function FontFace(family, source) {
    this.family = family;
    this.source = source;
    var self = this;
    this.load = function () { return Promise.resolve(self); };
  }, false);
  // wasm-bindgen 胶水用它调度 async 导出的 future pump：微信宿主**没有**这个全局
  // （模拟器实测 `typeof queueMicrotask === 'undefined'`，2026-10-12）。缺它时第一拍调度
  // 在微任务里抛 ReferenceError、无人接住，Rust 侧 QUEUE 就此失去驱动——
  // `probe_canvas_host`/`engine_host_begin` 表现为**静默挂死**（既不 ok 也不 failed）。
  // 语义对齐浏览器：callback 作为微任务执行，异常按未处理 Promise 拒绝上报。
  defineGlobal('queueMicrotask', function (callback) {
    if (typeof callback !== 'function') throw new TypeError('queueMicrotask: callback must be a function');
    Promise.resolve().then(callback);
  }, true);
  // 微信宿主同样**没有** `WebGL2RenderingContext` 全局（模拟器实测两个大小写变体都是
  // undefined，2026-10-12）。这专杀 wasm-bindgen 的向下转型：胶水对 wgpu-hal 取到的
  // webgl2 上下文执行 `arg0 instanceof WebGL2RenderingContext`（DOM 大小写；见胶水
  // `__wbg_instanceof_WebGl2RenderingContext_*`），全局缺失时 ReferenceError 被胶水自己的
  // try/catch 吞掉、结果判 false → `dyn_into::<WebGl2RenderingContext>().expect(...)` 在
  // `wgpu-hal/src/gles/web.rs:89` panic（六问探针实测 2026-10-12）。装法必须取**原生上下文
  // 自己的构造器**（胶水拿到的 ctx 就是它造的，instanceof 必真），鸭子类型假类骗不过
  // 真实原型链以外的判据、还会把预检读数变成自证。两个大小写都挂：web_sys 的 Rust 名是
  // `WebGl2RenderingContext`，将来别的转型路径可能查那一边。
  (function installWebGl2Constructor() {
    if (typeof globalThis.WebGL2RenderingContext === 'function') return;
    try {
      var probeCanvas = null;
      if (typeof wx.createOffscreenCanvas === 'function') {
        try { probeCanvas = wx.createOffscreenCanvas({ type: 'webgl', width: 1, height: 1 }); } catch (e) { probeCanvas = null; }
      }
      if (!probeCanvas || typeof probeCanvas.getContext !== 'function') {
        var again = safeCall(wx, 'createCanvas');
        probeCanvas = again && again !== canvas && typeof again.getContext === 'function' ? again : null;
      }
      var gl = probeCanvas ? probeCanvas.getContext('webgl2') : null;
      var ctor = gl && gl.constructor;
      if (typeof ctor !== 'function') {
        warn('wechat-adapter: 宿主给不出 webgl2 上下文，WebGL2RenderingContext 全局无法安装（GPU 路线会停在 wgpu 转型处，这是宿主能力缺失而不是 shim 缺失）');
        return;
      }
      defineGlobal('WebGL2RenderingContext', ctor, true);
      defineGlobal('WebGl2RenderingContext', ctor, true);
      log('wechat-adapter: 已装 WebGL2RenderingContext 全局（取自原生 webgl2 上下文，wasm 侧 dyn_into 依赖它）');
    } catch (error) {
      warn('wechat-adapter: 安装 WebGL2RenderingContext 全局失败 -> ' + hostError(error));
    }
  })();
  defineGlobal('requestAnimationFrame', function (callback) {
    // 帧回调之后再画覆盖层/警示条：运行时通常每帧 clearRect 一次，画在它前面等于白画。
    var wrapped = function (time) {
      var result;
      try {
        result = callback(time || Date.now());
      } finally {
        try { afterFrame(); } catch (e) { /* 自证逻辑绝不影响游戏主循环 */ }
      }
      return result;
    };
    if (nativeRaf) return wx.requestAnimationFrame(wrapped);
    return setTimeout(function () { wrapped(Date.now()); }, 16);
  }, false);
  defineGlobal('cancelAnimationFrame', function (id) {
    if (nativeRaf) return wx.cancelAnimationFrame(id);
    return clearTimeout(id);
  }, false);
  defineGlobal('localStorage', {
    getItem: function (key) {
      var value = safeCall(wx, 'getStorageSync', String(key));
      return (value === '' || value === undefined || value === null) ? null : String(value);
    },
    setItem: function (key, value) {
      // 必须把成败**返回**给运行时：只 warn 不上报的话，`save.commit()` 拿到的永远是"写成功"，
      // 玩家看到的还是"存了但重开没恢复"（与本轮修掉的 shim 静默失败同一个形状）。
      try { wx.setStorageSync(String(key), String(value)); return true; }
      catch (e) { warn('wechat-adapter: 存档写入失败 ' + e); return false; }
    },
    removeItem: function (key) { try { wx.removeStorageSync(String(key)); } catch (e) { /* 忽略 */ } },
    clear: function () { try { wx.clearStorageSync(); } catch (e) { /* 忽略 */ } }
  }, false);

  installTouchBridge();
  installLifecycle();

  // ---------------------------------------------------------------- 启动

  // ---------------------------------------------------------------- 启动看门狗
  //
  // `boot()` 是异步的，而"卡在某一步"与"跑起来了却没画"在日志里长得**一模一样**：
  // 实测贴图 `src` 没真正交给宿主时，运行时的 `loadSceneImages` 永不 settle →
  // 屏幕全黑、控制台 0 error、0 帧，而日志看上去一切正常（这就是 "流程正常但画面全黑"
  // 那一类故障里最难查的一种）。超时后把**诊断画到屏幕上**，不依赖控制台也不依赖帧循环，
  // 并把最可能的原因直接写出来；若之后启动真的完成了，第一帧就会把它清掉。
  var BOOT_TIMEOUT_MS = 10000;
  // 超时可现场覆盖（`wx.setStorageSync('rustlua_boot_timeout_ms', 300)`）：慢 CDN 上可以放大，
  // 自动化用例里可以缩短——否则看门狗本身没法被验证（等 10 秒的用例进不了质量门）。
  var BOOT_TIMEOUT_KEY = 'rustlua_boot_timeout_ms';
  var bootStartedAt = 0;
  var bootWatchdog = null;
  // 当前启动阶段。看门狗原本只会说"启动超时：Lua 主循环没有起来"，而卡在**分包下载**、
  // **wasm 实例化**与卡在 Lua 首帧是三件完全不同的事（前者与游戏逻辑毫无关系），
  // 现场只拿到同一句话等于没拿到线索。阶段写进 DOM 属性，真机与无头都能读。
  var bootPhase = 'init';

  function setBootPhase(phase) {
    bootPhase = phase;
    bodyElement.setAttribute('data-rustlua-boot-phase', phase);
  }

  function bootTimeoutMs() {
    var value = parseInt(safeCall(wx, 'getStorageSync', BOOT_TIMEOUT_KEY), 10);
    return value > 0 ? value : BOOT_TIMEOUT_MS;
  }

  // wasm 实例化超时单独一个键：它与"Lua 主循环没起来"是两件事，判定阈值也该能分开调
  // （分包刚挂载完就实例化，字节在本地，15 秒还很宽裕；慢设备上可以现场放大）。
  var WASM_TIMEOUT_MS = 15000;
  var WASM_TIMEOUT_KEY = 'rustlua_wasm_timeout_ms';

  function wasmTimeoutMs() {
    var value = parseInt(safeCall(wx, 'getStorageSync', WASM_TIMEOUT_KEY), 10);
    return value > 0 ? value : WASM_TIMEOUT_MS;
  }

  function stallHint() {
    var ok = assetStats.imageLoaded || 0;
    var failed = assetStats.imageFailed || 0;
    var missing = assetStats.imageMissing || 0;
    if (frameCount > 0) return '已经有 ' + frameCount + ' 帧在跑，卡点在 Lua 侧（看控制台上的异常）。';
    if (ok === 0 && failed === 0 && missing === 0) {
      // 两个可能：src 没交到宿主（宿主不开始加载），或者交到了但回调传不回来（宿主的回调桥
      // 接在私有字段上、被 own accessor 盖断）。两者症状一模一样，所以两条接管方式都要报出来。
      return '贴图一张都没有回调（ok/fail/未供应 全为 0）：要么 Image.src 没交到宿主、' +
        '要么回调传不回运行时（src接管=' + (srcTakeover || 'none') + ', 回调接管=' + (callbackTakeover || 'none') + '）' +
        '，运行时的贴图 Promise 永远不 settle。';
    }
    return '贴图 ok=' + ok + ' fail=' + failed + ' 未供应=' + missing + '：贴图是有回调的，卡点在别处（看远端下载与资源读取自检）。';
  }

  function reportBootStall() {
    var elapsed = Math.round((Date.now() - bootStartedAt) / 1000);
    var wasmNote = (bootPhase === 'wasm' || bootPhase === 'pack')
      ? ' wasm=' + wasmState.status + (wasmState.source ? '/' + wasmState.source : '')
      : '';
    var message = '启动超时 ' + elapsed + 's（阶段 ' + bootPhase + '）' + wasmNote + '：' + stallHint();
    console.error('[wechat-adapter] ' + message + ' 现场：' + JSON.stringify(diagSnapshot()));
    publishDiag();
    showBootError(message);
  }

  var started = false;
  // **宿主直启**（B8 第一步；默认关，__RUSTLUA_HOST_DIRECT_BOOT === true 时才走）。
  //
  // 它要替掉旧世界 web-runtime.js 的启动编排。查实后这一步**很小**，因为原语都在：
  //   * Lua 运行时已由 scene 档的预装步骤建好（lua_boot_with_reader，见上面"预装 reader provider"）；
  //   * lua_execute_main() 自己会装 **require 搜索器**（install_web_require，vfs provider -> pack::decode），
  //     所以旧世界那段"把所有 scripts/**.lua 读进 JS 再注册 package.preload"**在这里是多余的**；
  //   * 帧泵 / boot 属性 / 失败态都已归适配层（B5/B6 查实）。
  // 旧世界存在的最后理由是"**先注入全部宿主补丁、再执行 main.lua**"这个顺序（lua_bridge.rs:492），
  // 而那个顺序约束**是 JS 补丁带来的** —— 补丁没了，约束也就没了。
  //
  // 风险面（正是这一步要暴露的）：补丁没了以后，游戏看到的是**引擎自己的** API 表（那正是 B8 的目标形态），
  // 于是"补丁特有的行为"会在这一步现形，而不是在删掉 414 KB 之后。
  function hostDirectBoot() {
    var bridge = globalThis.__RUSTLUA_LUA_BRIDGE;
    if (!bridge || typeof bridge.lua_execute_main !== 'function') {
      throw new Error('宿主直启缺少 lua_execute_main（桥未就绪：web-runtime.js 被跳过时没人装它）');
    }
    bridge.lua_execute_main();
    // **留下可读证据**：光看"游戏跑起来了"分不出走的是直启还是旧世界（两条路都会跑起来）。
    // 这个标记进 `p0-probe.js` 的读数，于是"确实没经 web-runtime.js"是被**观测**到的，而不是推断的。
    try { globalThis.__RUSTLUA_HOST_DIRECT_BOOT_DONE = true; } catch (error) { /* 无全局对象 */ }
    log('wechat-adapter: 宿主直启完成（main.lua 由引擎执行，未经 web-runtime.js）');
    return true;
  }

  function start(boot) {
    if (started) return Promise.resolve(false);
    started = true;
    bodyElement.setAttribute('data-rustlua-adapter', 'ready');
    setBootPhase('splash');
    // 固定主包路径，不等 pack/远端/wasm。接管形态绕过 2D 改道，在同一张可见画布
    // 一次性绘制 WebGL2；helper 清理并恢复 GL 状态之后才允许引擎拿到上下文。
    var showStartupBranding;
    try { showStartupBranding = require('./startup-branding.js'); }
    catch (error) {
      warn('wechat-adapter: startup splash helper unavailable -> ' + hostError(error));
      return Promise.resolve().then(function () { return startAfterSplash(boot); });
    }
    return showStartupBranding({
      canvas: canvas,
      getContext: rawGetContext,
      gpu: engineVisibleTakeover,
      createImage: function () { return wx.createImage(); },
      requestFrame: typeof wx.requestAnimationFrame === 'function'
        ? function (callback) { wx.requestAnimationFrame(callback); }
        : (typeof canvas.requestAnimationFrame === 'function'
          ? function (callback) { canvas.requestAnimationFrame(callback); } : null),
      warn: warn
    }).then(function () { return startAfterSplash(boot); });
  }

  function startAfterSplash(boot) {
    // 看门狗从**这一行**就开始计时，覆盖"分包下载 → wasm 实例化 → 字体 → Lua boot"整条链。
    // 原来的写法是在 wasmReady 兑现**之后**才 arm：宿主如果有一步既不 success 也不 fail
    // （WXWebAssembly.instantiate 与 loadSubpackage 真机上都出现过这种半死状态），
    // 卡在那里的现场是黑屏 + 0 error + 连超时提示都没有——最难查的一种。
    bootStartedAt = Date.now();
    setBootPhase('pack');
    if (!bootWatchdog) bootWatchdog = setTimeout(reportBootStall, bootTimeoutMs());
    // 必须在 ensurePack 之前读：它决定"分包加载失败是否致命"。
    loadRemoteConfig();
    updateAssetMarkers();
    // 覆盖层开关：发布目录放 debug-overlay.txt，或 wx.setStorageSync('rustlua_debug_overlay', '1')。
    // 放在这里（而不是模块加载时）是因为它要读文件/存储，且读法依赖已探明的路径方言。
    try {
      debugOverlayEnabled = debugOverlayRequested();
    } catch (error) {
      debugOverlayEnabled = false;
      warn('wechat-adapter: 覆盖层开关读取失败 -> ' + hostError(error));
    }
    if (debugOverlayEnabled) {
      // 覆盖层要读运行时每帧自报的那些 `data-rustlua-*` 属性（世界状态/预设文字/九宫格切片），
      // 而它们在发布包里默认不写（那是每帧两次跨 wasm 边界的 Lua 调用加若干次 setAttribute，
      // 见 web-runtime.js 的 DIAGNOSTICS_ON）。覆盖层是调试工具，它开启时就把诊断一并打开，
      // 否则覆盖层会显示成"世界=空、文字=0/0"——一个看起来像游戏坏了、其实是自己没要数据的现场。
      // 时序成立：这里仍在 boot() 之前，而 web-runtime.js 是在 boot 里才加载的。
      g.__RUSTLUA_DIAGNOSTICS = true;
      log('wechat-adapter: 渲染自证覆盖层已开启（覆盖层不会随游戏画面清屏消失）');
      // boot 之前先做一次像素读写自检：证明"这张画布此刻可读可写"。
      // 注意：这里画的图案会被 boot 自己的清屏擦掉，所以**不能**把"上屏自证"押在这上面，
      // 真正留痕的那一次在 boot 完成后（drawStartupProbe）。
      try {
        var probeCtx = canvas.getContext('2d');
        if (probeCtx) {
          pixelProbeValue = probePixels(probeCtx);
          log('wechat-adapter: 启动前像素自检 ' + pixelProbeValue);
        }
      } catch (error) {
        warn('wechat-adapter: 启动前像素自检失败 -> ' + hostError(error));
      }
    }
    // 首屏清单要在**任何** pack/ 读取之前就位（font 与 probe 都会读 pack/）。这里显式加载一次
    // 而不是只靠 readWithStrategies 里的惰性加载：清单缺失是"发布形态不对"的硬信号，值得一条日志。
    if (loadFirstScreenManifest()) {
      bodyElement.setAttribute('data-rustlua-layout', 'mainpack+pack');
      bodyElement.setAttribute('data-rustlua-mainpack-entries', String(firstScreen.count));
    }
    // 资源读取自检：分包形态下这三个探针在 mainpack/ 下，验证的正是**路由层本身**。
    probePackReadability();
    // wasm 核心：分包形态下它是 `pack/game_web_bg.wasm`（在**分包**里），其余形态是
    // `pkg/game_web_bg.wasm`（主包）。Lua 执行层就在这个 wasm 里（WP10 迁移后无备用解释器）：
    // 加载失败时宿主交互的诊断通道（`data-rustlua-wasm` / `__RUSTLUA_DIAG().wasm`）会如实记录，
    // 而 `__RUSTLUA_LUA_BRIDGE` 缺失会让 web-runtime.js 抛 `wasm lua bridge not loaded`
    // （发布形态问题，不降级）。
    // **它返回 Promise**：小游戏宿主的 `WXWebAssembly.instantiate(代码包路径, imports)` 只有
    // 异步形态，所以这里只发起加载、把它排在 boot 之前完成（见下面的 wasmReady 链）。
    var wasmInPack = isWasmInPack();
    var wasmReady = null;
    if (wasmInPack) {
      // 内核在分包里 ⇒ **boot 必须排在分包挂载之后**（次序变了，这是本次搬迁的代价）：
      // 挂载期间由 JS 自己画进度条，因为运行时还没起来、Lua 一行都没跑。
      // 挂载失败**不再**是"标题照常出、玩法缺素材"——那时 instantiate 读不到内核，
      // boot 会失败并走 showBootError；文案由 drawPackFailure 按形态切换。
      startPreBootUi();
      wasmReady = ensurePackForBoot(PACK_BOOT_ATTEMPT_LIMIT - 1).then(function (mounted) {
        if (!mounted) {
          // 立刻补画一次失败横幅，**再**停帧链：这一刻之后没有人驱动 afterFrame，
          // 而"分包没挂上"这件事在真机上必须有可见解释（屏幕是唯一通道），不能只剩控制台。
          // 此刻 `data-rustlua-boot` 还没被判 failed，横幅不会被 afterFrame 的抑制逻辑挡掉。
          var failCtx = null;
          try { failCtx = canvas.getContext('2d'); } catch (ctxError) { failCtx = null; }
          withAdapterDraw(failCtx, drawPackFailure);
          stopPreBootUi();
          warn('wechat-adapter: 分包没挂载，而 wasm 内核就在分包里 —— 实例化将失败（path-instantiate-failed），启动到此为止');
        } else {
          stopPreBootUi();
        }
        setBootPhase('wasm');
        return loadWasmCoreOnce();
      });
    } else {
      setBootPhase('wasm');
      wasmReady = loadWasmCoreOnce();
    }
    // 画布与游戏版面的自检：`game.json` 的方向与画布尺寸必须和 `ui/layout.lua` 的设计分辨率
    // 对得上。**为什么值得单独自检**：Lua 面板画在屏幕像素空间、不做缩放，画布尺寸不对的表现是
    // "面板溢出画布"（边缘控件看不见），而这在无头验证里**不会报错**
    // （假 ctx 不裁剪、超界坐标照样计数）→ 只能靠这一行把它变成可读的证据。
    reportViewAgreement();
    // 引擎侧画布探针：只在显式要求时跑（默认关），且**不 await**——它要建 wgpu 栈，不该拖住启动。
    // **必须挂在 `wasmReady` 之后**：`engineHostRef` 是 `attachWasmBridge` 里存的，而这一步在
    // 内核实例化 Promise 内部；在这里同步调用会让探针拿到 null 宿主、把 gpu2d 内核误报成
    // `unavailable`（第一版就是这么错的，靠 `kernel-not-loaded-yet` 这个新状态暴露出来）。
    wasmReady.then(function () {
      try { probeEngineCanvas(); } catch (error) { /* 探针自身绝不掀翻启动 */ }
      // 引擎宿主：同样挂在 `wasmReady` 之后（要 `engineHostRef`），同样**不 await**——
      // 建 wgpu 栈不该拖住启动；`running` 之后由 scene 档自泵驱动（原子模式默认走这条），
      // 非 scene 档才由 afterFrame 每帧驱动。
      try { engineHostStart(); } catch (error) { /* 宿主自身绝不掀翻启动 */ }
    });
    // 分包载入。**内核在分包里时上面已经 await 过同一条 Promise**（这里只是补状态标记，
    // ensurePack 内部共享同一个 packReady，不会重复下载）；内核在主包时这才是"后台下载"：
    // 首屏资源全在主包里，boot 不等它，分包只影响"进玩法场景"那一步
    // （那时的 pack/ 读取会 await 它，见 fetchShim / Image.src）。
    // 失败不能掀翻启动 Promise（见 ensurePack），否则标题已经在屏上却要被整屏错误页盖掉。
    ensurePack().then(function (ok) {      bodyElement.setAttribute('data-rustlua-pack', ok ? 'ready' : packState);
      if (ok) {
        bodyElement.setAttribute('data-rustlua-pack-mount-frame', String(packMountFrame));
        bodyElement.setAttribute('data-rustlua-pack-progress-events', String(packProgressEvents));
        log('wechat-adapter: 分包就绪，首屏之外资源可从 ' + (packSubpackageName() || PACK_SUBPACKAGE) + '/ 读取');
      }
    }).catch(function (error) {
      // ensurePack 自身不 reject；这里只兜住"上面的 then 里抛了"这种意外，
      // 绝不让它变成 unhandled rejection（那在小游戏里表现为静默）。
      warn('wechat-adapter: 分包后台载入异常 -> ' + hostError(error));
    });
    return wasmReady
      .then(function () {
        // wasm（= Lua 执行层）挂载完成之后才允许 boot：`window.__RUSTLUA_LUA_BRIDGE` 必须在
        // web-runtime.js 走到 Lua 之前存在，否则报的是"桥缺失"这种发布形态错误。
        setBootPhase('font');
        return loadGameFont();
      })
      .then(function () {
        setBootPhase('lua-boot');
        // **B8：唯一路径。** 旧世界（`web-runtime.js`）已删除，启动编排在这里完成：
        // reader 预装（scene 档那一步）+ `lua_execute_main()` + 宿主帧泵。`boot` 形参仍保留，
        // 是为了让调用方（`game.js`）保有一条"被要求跑旧世界就**响亮失败**"的守卫。
        return hostDirectBoot();
      })
      .then(function () {
        if (bootWatchdog) { clearTimeout(bootWatchdog); bootWatchdog = null; }
        setBootPhase('running');
        log('wechat-adapter: 启动完成');
        // 关键：**启动完成后立刻**画一次自证图案，不依赖帧循环。真机上最常见的黑屏就是
        // "流程走完、一帧都没画"，那时只有这一次留痕能证明"像素上得了屏"（实测：
        // 放在 boot 之前画会被 boot 的清屏擦掉，等于没画）。
        drawStartupProbe();
        return true;
      })
      .catch(function (error) {
        // 失败路径必须撤掉看门狗：否则 10 秒后它的超时提示会盖掉真正的失败原因。
        if (bootWatchdog) { clearTimeout(bootWatchdog); bootWatchdog = null; }
        var message = (error && error.message) ? error.message : String(error);
        // wasm 失败的分层原因必须出现在**错误行与屏幕本身**，而不是只进 console.warn：
        // 开发者工具的"问题"面板只收 console.error，于是现场只剩一句 `wasm lua bridge not loaded`
        // ——而这句话对四种完全不同的塌法（宿主没有 WXWebAssembly / 只有 instantiate 没有
        // Module 构造器 / wasm 字节读不到 / 模块被拒绝编译）给不出任何区分。
        // 实测 2026-09-30 开发者工具就是这样：本机全绿的产物在工具里报同一句，只能靠人
        // 去控制台敲 `__RUSTLUA_DIAG().wasm` 才拿得到判据。真机没控制台，屏幕就是唯一通道。
        if (wasmState.status !== 'ok') {
          message = message + ' [wasm=' + wasmState.status
            + (wasmState.source ? '/' + wasmState.source : '')
            + (wasmState.detail ? ' detail=' + String(wasmState.detail).slice(0, 200) : '')
            + (wasmState.attempts && wasmState.attempts.length
              ? ' tried=' + String(summarizeAttempts(wasmState.attempts)).slice(0, 220) : '') + ']';
        }
        console.error('[wechat-adapter] 启动失败:', message);
        bodyElement.setAttribute('data-rustlua-boot', 'failed');
        bodyElement.setAttribute('data-rustlua-boot-detail', message.slice(0, 400));
        showBootError('启动失败: ' + message);
        drawStartupProbe();
        throw error;
      });
  }

  // WASM 说明（WP10 迁移后）：代码包里发行 `pkg/game_web.js`（胶水，恒在主包）+ wasm 内核本体
  // （位置随形态变：分包形态在 `pack/game_web_bg.wasm`，其余在 `pkg/game_web_bg.wasm`，
  //  判据与原因见文件头 WASM_MODULE_PATH_PACK 那段）。
  // 胶水用 **`--target no-modules`** 生成（纯脚本、无 import/export），由 `wasm-bridge.js`
  // 包成 CommonJS。**实例化只能走宿主的代码包路径通道**：微信的 `WXWebAssembly` 只暴露
  // `instantiate(path, imports) -> Promise`，第一个参数**只接受代码包路径字符串**
  // （实测开发者工具对字节数组报 `WebAssembly.instantiate: first argument must be a string`；
  // 文档也未提供 Module/Instance 构造器）。字节 `initSync` 那条只在标准 WebAssembly 宿主
  // （本机 Node）成立，保留为兜底通道，且诊断里 `channel` 会写明实际用了哪条。
  // **不复用 webgl 那份 `--target web` 的 ESM 胶水**：小游戏按 CommonJS 解析，
  // 顶层 `export` 会让启动直接 SyntaxError。
  //
  // 当前边界（不得夸大）：wasm 即 Lua 执行层（mlua 5.4 + vendored），`__RUSTLUA_LUA_BRIDGE`
  // 承载全部 Lua 交互；宿主交互诊断通道（`data-rustlua-wasm` / `__RUSTLUA_DIAG().wasm`）如实记录。
  // `game_web::start_wechat`（去 DOM 入口）**未接线、未验证**，状态见
  // `engine/crates/game_web/src/lib.rs` 与 `plugins/wechat-minigame/README.md`。
  // ---------- 画布与游戏版面的自检（M32） ----------
  //
  // 读包内 `project.toml` 的 `window_width/height`（那是工程声明的设计分辨率，与
  // `scripts/ui/layout.lua` 的 DESIGN_W/DESIGN_H 同源，由 `ui/layout_check.lua` 在原生侧守住），
  // 与适配层自己设的画布尺寸比对。不一致时**响亮 warn + 进诊断**：这是"设计分辨率与画布
  // 尺寸错配"
  // 这类错配唯一能在无头环境里被发现的地方（画面不会被裁剪器报错，只会默默溢出）。
  function reportViewAgreement() {
    var declaredWidth = null;
    var declaredHeight = null;
    // 逻辑路径**必须带 `pack/` 前缀**：`readSubjects` 对不带前缀的路径**原样返回**（既不查
    // 改名表 `hostFileFor`、也不按首屏清单路由到 `mainpack/`），于是读的是代码包根下并不存在
    // 的 `project.toml`——分包形态下它的真身是 `mainpack/project.toml.txt`，结果**恒报 unknown**，
    // 这道"设计分辨率与画布不一致"的守卫等于没装。
    // 实测：画布 720×1280 对声明 1280×720 本该报 mismatch，实际却报 unknown——横竖屏错配
    // 一直没被发现，根因就在这里。
    try {
      var toml = readWithStrategies('pack/project.toml', readTextRaw);
      if (toml && typeof toml.value === 'string') {
        var mw = /window_width\s*=\s*(\d+)/.exec(toml.value);
        var mh = /window_height\s*=\s*(\d+)/.exec(toml.value);
        if (mw && mh) {
          declaredWidth = Number(mw[1]);
          declaredHeight = Number(mh[1]);
        }
      }
    } catch (error) { /* 读不到就按"没声明"记，不当作错误 */ }

    var known = declaredWidth > 0 && declaredHeight > 0;
    var agree = known ? (declaredWidth === VIEW_WIDTH && declaredHeight === VIEW_HEIGHT) : null;
    viewAgreement = {
      declared: known ? (declaredWidth + 'x' + declaredHeight) : 'unknown',
      canvas: VIEW_WIDTH + 'x' + VIEW_HEIGHT,
      agree: agree,
      // 方向**只报编译期常量**，不去读 `game.json`：那个文件在真机上 `readFileSync` 一律
      // `permission denied`（`*.js`/`game.json` 上传后被编译），而适配层有一条硬约束
      // **一次都不许读它**（回归守卫 `--expect-no-read game.json`）。
      // 第一版这里写了 `readText('game.json')` 取 deviceOrientation，被那条守卫当场抓住——
      // 守卫在这里是对的：拿一个真机读不到的输入去判"方向对不对"，本身就是把功能绑在不可用的输入上。
      // 方向属于**发布配置**，真值在 `plugins/wechat-minigame/game.json` 与构建期校验里。
      orientation: GAME_ORIENTATION
    };
    // 三档：ok（声明且一致）/ mismatch（声明但不一致）/ unknown（读不到声明，无从判断）。
    // `unknown` **不算错**：project.toml 可能因发布形态而不在包内，那不该报警。
    bodyElement.setAttribute('data-rustlua-view', agree === true ? 'ok' : (agree === false ? 'mismatch' : 'unknown'));
    if (agree === false) {
      warn('wechat-adapter: 画布尺寸与工程声明不一致（画布 '
        + VIEW_WIDTH + 'x' + VIEW_HEIGHT + '，project.toml ' + viewAgreement.declared
        + '）—— Lua 面板画在屏幕像素空间、不做缩放，会导致版面溢出画布');
    } else if (agree === true) {
      log('wechat-adapter: 画布 ' + VIEW_WIDTH + 'x' + VIEW_HEIGHT + '、方向 '
        + viewAgreement.orientation + '，与 project.toml 声明一致');
    } else {
      log('wechat-adapter: 画布 ' + VIEW_WIDTH + 'x' + VIEW_HEIGHT + '、方向 '
        + viewAgreement.orientation + '（读不到 project.toml 的 window_* 声明，跳过一致性判断）');
    }
  }

  // ---------- wasm 核心：加载 + 自证（WP10 迁移后） ----------
  //
  // 口径先说清：小游戏的**游戏逻辑**由 wasm 内的 mlua 执行层承载（与 webgl 发布同一条路径），
  // wasm 核心在这里做**三件事**——能读到、能实例化、导出清单对得上，并把 6 个 Lua 桥导出
  // 挂到 `window.__RUSTLUA_LUA_BRIDGE`（`lua_boot` 包装成 `lua_boot_with_reader`）。
  // 为什么值得做：如果没人验证它"真的装进来了"，Lua 启动失败会被当成"桥没挂上"的假象；
  // 所以这里把结果做成可核对的证据（桥缺失时 web-runtime.js 会抛硬错误，不是静默降级）。
  //
  // 结果档位（都进 `__RUSTLUA_DIAG().wasm` 与启动日志）：
  //   ok                        实例化成功，`channel` 标出用的是哪条通道，`facade` 标出标准
  //                             `WebAssembly` 域名的来源（转发的构造器清单，或 host-standard），
  //                             `exports` 是真实导出名清单
  //   no-webassembly            宿主没有 WXWebAssembly / WebAssembly（真机极旧基础库）
  //   webassembly-api-incomplete 宿主既没有 instantiate(path,imports) 也没有 Module/Instance 构造器；
  //                             或宿主无标准 `WebAssembly` 域名**且** `WXWebAssembly` 没有 `Tag`
  //                             （胶水的两条 tag 导入在求值期就要它，2026-10-03 补的判据）
  //   path-instantiate-failed    路径通道（小游戏的唯一公开形态）失败，attempts 里带每条候选路径的原因
  //   glue-missing / glue-shape-unexpected  胶水没打进包或形态不对（发布形态问题）。标准域名现在
  //                             由 wasm-bridge 按宿主转发，这句不再被"宿主没有 WebAssembly 全局"
  //                             冒充；detail 带求值期全局读数（TextEncoder/TextDecoder 缺一也在这里现形）
  //   bridge-missing            wasm-bridge.js 自身 require 失败
  //
  // **为什么是 Promise**：`WXWebAssembly` 只有异步的 `instantiate(路径, imports)`（文档如此，
  // 且字节数组会被直接拒），所以 wasm 挂载天生是异步的；boot 必须排在它之后
  // （见 start() 里的 loadGameFont 之前那一步）。
  var wasmState = { status: 'not-attempted' };
  var wasmLoadPromise = null;
  // EH 真机探针的读数（`eh_probe=…`）：单独存，**不**挂在 wasmState 上——loadWasmCoreOnce 里的
  // finish() 会整个替换 wasmState 对象，而探针与实例化并发、落点先后不定，挂上去会被 finish 抹掉。
  // 探针永不 reject，最迟 3s 后一定写进来（宿主不兑现也是 timeout），诊断读到它只是迟早。
  var ehProbeReadout = 'not-attempted';
  // 画布/版面一致性自检结果（见 reportViewAgreement），进 __RUSTLUA_DIAG().view_agreement。
  var viewAgreement = { declared: 'unknown', canvas: 'unknown', agree: null, orientation: 'unknown' };

  // 幂等：只真正加载一次，后续调用返回同一个 Promise（start() 与 api.loadWasm 共用）。
  function loadWasmCoreOnce() {
    if (wasmLoadPromise) return wasmLoadPromise;
    wasmLoadPromise = new Promise(function (resolve) {
      var bridge = null;
      try {
        bridge = require('./wasm-bridge.js');
      } catch (error) {
        finish({ status: 'bridge-missing', detail: hostError(error) });
        warn('wechat-adapter: wasm 桥接层加载失败 -> ' + wasmState.detail);
        resolve(wasmState);
        return;
      }
      // readBytes 只喂字节通道（有 Module/Instance 的宿主）；路径通道由宿主自己按
      // 代码包路径读 wasm，适配层不碰那 2.3 MB。
      // 实例化超时：宿主**既不 resolve 也不 reject** 时（真机上出现过），启动链会永远停在
      // 这一步——屏幕黑着、日志一条没有、也没有任何超时（看门狗虽然会报，但那是"卡住了"，
      // 而这里能给出"卡在哪条通道"）。超时后按实例化失败结算，让启动走失败路径报出原因。
      var settled = false;
      var timer = setTimeout(function () {
        if (settled) return;
        settled = true;
        finish({ status: 'instantiate-timeout', detail: '宿主 ' + wasmTimeoutMs() + 'ms 内未返回（既不成功也不失败）' });
        warn('wechat-adapter: wasm 实例化超时（' + wasmTimeoutMs() + 'ms）——按失败结算，启动会报出原因');
        resolve(wasmState);
      }, wasmTimeoutMs());
      bridge.loadWasmCore({
        readBytes: readBytes,
        wasmPath: wasmModulePath(),
        ehProbePath: EH_PROBE_PATH,
        onEhProbe: function (verdict) {
          ehProbeReadout = verdict;
          // 落 console：真机 vconsole 里能直接看到设备对 wasm EH 的裁决（accepted / reject[...] /
          // timeout），不必等 __RUSTLUA_DIAG 快照。诊断对象仍照旧存 ehProbeReadout（见上）。
          log('wasm-bridge: EH 真机探针裁决 = ' + verdict);
        },
        log: log
      }).then(function (result) {
        if (settled) return; // 已被超时结算：不再覆盖状态，也不再重复 resolve
        settled = true;
        clearTimeout(timer);
        if (result.ok) {
          // 顺序要紧：attachWasmBridge 把 luaBridge/hostApi 写在 wasmState 上，
          // 必须等 finish 换好对象之后再挂，否则诊断里这两个字段会被丢掉。
          finish({
            status: 'ok', source: result.source, channel: result.channel, path: result.path,
            facade: result.facade, exports: result.exports, bytes: result.bytes
          });
          attachWasmBridge(result);
        } else {
          finish({ status: result.reason, source: result.source, detail: result.detail, attempts: result.attempts });
          warn('wechat-adapter: wasm 核心未就绪（' + result.reason
            + (result.detail ? '：' + result.detail : '')
            + (result.attempts ? '｜通道尝试: ' + summarizeAttempts(result.attempts) : '') + '）'
            + '——Lua 执行层在 wasm 里（WP10 迁移后无备用解释器），`__RUSTLUA_LUA_BRIDGE` 缺失会让'
            + ' web-runtime.js 抛硬错误（发布形态问题，不降级）；"包里带了 wasm 却没装上"正是这种问题');
        }
        resolve(wasmState);
      }, function (error) {
        // 桥接层 reject 也要结算：原来的写法没有这条分支，rejection 会一路冒到 start()，
        // 现场只剩一句笼统的启动失败，而"哪条通道、为什么"全在 wasmState 里没被写进去。
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        finish({ status: 'bridge-rejected', detail: hostError(error) });
        warn('wechat-adapter: wasm 桥接层 reject -> ' + hostError(error));
        resolve(wasmState);
      });
    });
    return wasmLoadPromise;
  }

  function summarizeAttempts(attempts) {
    return (attempts || []).map(function (attempt) {
      return attempt.channel + (attempt.path ? '(' + attempt.path + ')' : '')
        + '=' + (attempt.detail || attempt.reason || 'failed');
    }).join(' ; ');
  }

  function finish(state) {
    wasmState = state;
    bodyElement.setAttribute('data-rustlua-wasm', state.status);
    return wasmState;
  }

  function attachWasmBridge(result) {
    // WP10 迁移（fengari → mlua-wasm）：wasm 导出 Lua 桥函数 → 挂到
    // window.__RUSTLUA_LUA_BRIDGE，web-runtime.js 的 Lua 交互全部经它完成。
    // 微信没有 wasm 预加载资源表（webgl 的 start() 才有），因此 `lua_boot`
    // 包装成 `lua_boot_with_reader(同步读取器)`：适配层 readText 即该读取器
    //（pack/ 明文，vfs provider 原样交给引擎 pack::decode）。
    // 导出缺失时保持"未就绪"状态（boot 会如实报错），不参与 success 判定。
    if (!result.host) { wasmState.luaBridge = 'no-glue-export'; wasmState.hostApi = 'no-glue-export'; return; }
    // 引擎侧画布探针要用同一个胶水对象上的 `probe_canvas_host`（只在 `gpu2d` 档存在）。
    // 存引用而不是现在就探测：探测是显式开关驱动的动作，且不该拖住启动。
    engineHostRef = result.host;
    try {
      // wasm 侧 vfs 的路径形如 `/project/scripts/main.lua`；适配层 readText 只认
      // `pack/` 前缀的逻辑路径（readSubjects 据此路由到 mainpack/ 首屏清单 + 改名表
      // hostFileFor）。包装器负责转换：剥 `/project/` → 补 `pack/` 前缀。
      function wechatReadFile(path) {
        var rel = String(path).replace(/^\/project\//, '');
        if (rel.indexOf('pack/') !== 0) rel = 'pack/' + rel;
        // 按**字节**返回（0.7.11）：utf8 字符串通道会把 TTF/PNG 的非法序列静默换成
        // U+FFFD（实测：贴图 0/14 全失败 + 字体 InvalidFont，而 .lua 文本毫发无损）。
        // wasm 侧 provider 认 ArrayBuffer/Uint8Array，字符串只作旧通道回落。
        var bytes = readBytes(rel);
        if (bytes) return bytes;
        return readText(rel, true);
      }
      wechatReadFileRef = wechatReadFile;
      window.__RUSTLUA_LUA_BRIDGE = {
        lua_boot: function () { return result.host.lua_boot_with_reader(wechatReadFile); },
        lua_do_string: result.host.lua_do_string,
        // 逐帧写 `__rustlua_dt` 用（不编译 chunk）。**门面是显式白名单**：Rust 侧加了导出
        // 却忘了加到这里，运行时就会拿到 undefined——真机上是"启动即失败"，
        // 而报错原文只有一句 is not a function。见 plugins/webgl/index.html 同一份门面。
        lua_set_number: result.host.lua_set_number,
        // WP7-79：**批量数值**（一次跨界写多个全局，相机三元组每帧走这里）。
        // 门面是显式白名单 ⇒ 这里少一行，运行时就会拿到 undefined（真机上是"启动即失败"）。
        lua_set_numbers: result.host.lua_set_numbers,
        // 每帧输入批量线格式（`RLFI`）：取代"每帧拼一段 Lua 源码再编译"。
        // 与 webgl 那份门面必须同步（同一份 web-runtime.js 跑在两个平台上，
        // 少一项就是"这个平台启动即失败"）。
        // WP7-58：**输入注入的唯一所有者是运行时**（设备层在它那边），所以这里只记账、
        // **不**再转交给宿主。转交等于给同一批输入造第二个所有者：宿主接管固定步时，
        // 同一帧输入会被注入两次（各自看着都正常，只有手感不对）；而"运行时跳过 + 宿主也没拿到"
        // 的组合更糟——第 88 轮就留下了那个 bug（`hostTick` 时整段跳过 ⇒ 宿主拿到 undefined
        // ⇒ 整局对按键没反应）。这批计数就是门用来钉"注入恰好来自一处"的证据。
        // **本笔复核（原子模式默认接管画面之后）：这条归属不变、也不该变。** 返回值 `touched`
        // 是 wasm 侧真的改了几位（`input frames=N bits=M`；恒 0 = 两边对"这一帧该变哪些位"理解
        // 不一致的第一手线索）。把注入搬到宿主，等于让宿主在**下一帧**才解码这一批 ⇒ 这一帧的
        // `touched` 拿不到真值 ⇒ 那个守卫读数变成恒 0 的**假信号**。宿主那条帧路
        // （`engineHostDriveFrame`）继续恒传 `undefined`，所以 `engine_host_input_applied` 恒 0
        // 就是"宿主不是第二个注入者"的证据；"输入所有权搬给宿主"属于删掉旧 JS 世界那一笔。
        lua_input_frame: function (bytes) {
          // B4 · 输入所有权交接：**原子模式下运行时不再是注入者**。
          //
          // 为什么必须停掉而不是"两边都写"：`LATEST_FRAME` 是**一个槽**，两个写者按帧交错写进去
          // ⇒ 谁后写谁赢，而两者的内容与时机都不同（运行时按它自己的采样帧、宿主按
          // `engineHostDriveFrame` 的帧）⇒ 输入会**帧间跳变**（按键时灵时不灵），而读数上只
          // 看得到"两边都在工作"。所以交接必须是原子的：一个模式，一个所有者。
          //
          // 返回值契约：运行时的 `appliedBits` 拿它当"两边对『这一帧该变哪些位』理解不一致"的
          // 守卫（恒 0 = 异常）。原子模式下这一批**不再被应用**，所以如实回 0；同时**不再累加**
          // `runtimeInputFrames`——那个计数现在的含义是"旧通道还被用了多少次"，恒 0 就是交接
          // 完成的判据（门按新契约断言：改看宿主的 `hostInputFrames`）。
          // **B4 输入所有权交接**：只在宿主**真的在跑**时才停掉旧通道。
          //
          // 为什么条件必须是"host running"而不是"原子模式"：离线假宿主不给 GL ⇒ 宿主起不来 ⇒
          // 若此时也停旧通道，就变成**两边都没注入**（世界不动），而读数上只看到
          // `runtime_input_frames=0` 像是"交接完成"——**这正是最危险的那种静默失效**。
          // 真机上宿主 `running` ⇒ 交接生效、一个所有者；这里离线 ⇒ 旧通道继续注入、世界照动。
          if (engineHost.status === "running") return 0;
          engineHost.runtimeInputFrames += 1;
          return result.host.lua_input_frame(bytes);
        },
        input_frame_layout_json: result.host.input_frame_layout_json,
        lua_call_global: result.host.lua_call_global,
        // WP7-13：固定步时钟的权威在宿主 wasm（`lua_bridge.rs`，内部是
        // `rustlua_infra::FrameClock`——与引擎宿主同一个策略模块）。JS 每帧只问"这一帧跑几步"，
        // 不再自持累积器——两个平台的追帧/丢帧口径因此不可能漂移。
        // 这里**包一层**只为一个读数：宿主接管固定步之后老世界还在跑的话，世界会被推进两次，
        // 而"两边各自都正常"的读数完全看不出这件事（见 engineHost.tickConflict）。
        lua_fixed_frame: function (elapsedSeconds) {
          engineHost.runtimeFixedFrames += 1;
          return result.host.lua_fixed_frame(elapsedSeconds);
        },
        // WP7-56：固定步的**执行**（虚拟键 → `__rustlua_dt` → `update()`）只有 wasm 一份实现，
        // 运行时的固定步阶段直接调它。桥白名单是**逐个列名**的，所以 wasm 侧每多一个运行时依赖的
        // 导出，这里与 `plugins/webgl/index.html` 的名单都必须同步——漏一个的表现是
        // `data-rustlua-boot=failed` + 一行"wasm 缺 lua_fixed_run_steps 导出"（具名，好定位；
        // 这正是第 87 轮踩到的那次）。
        // WP7-57：包一层计数——**这个**才是"运行时真的推进了世界"的证据（`lua_fixed_frame` 只是
        // 每帧问步数）。宿主接管固定步时这里若还被调用，就是双步 ⇒ 判 failed 并把 tick 还回去。
        lua_fixed_run_steps: function (steps, deltaSeconds) {
          engineHost.runtimeRunSteps += 1;
          return result.host.lua_fixed_run_steps(steps, deltaSeconds);
        },
        lua_fixed_steps_dropped: result.host.lua_fixed_steps_dropped,
        lua_fixed_accumulator_seconds: result.host.lua_fixed_accumulator_seconds,
        lua_fixed_reset_clock: result.host.lua_fixed_reset_clock,
    // WP7-30：JS 宿主问引擎要"这个名字对应的引擎句柄"（两个 id 空间的换算口）。
    lua_entity_handle: result.host.lua_entity_handle,
        // 无参无返回的全局钩子（每步的虚拟键注入 / 边沿清理）：取代
        // `doString("__rustlua_apply_virtual_input()")` 那种"为调无参函数而编译源码"的写法。
        lua_call_global_void: result.host.lua_call_global_void,
        lua_register_js_callback: result.host.lua_register_js_callback,
        lua_missing_globals: result.host.lua_missing_globals,
        lua_execute_main: result.host.lua_execute_main,
        // API 一致性门用（见 web-runtime.js 的 engineHostGlobals / __rustlua_export_api_surface）：
        // 前者给"桌面有哪些宿主全局"的唯一来源，后者导出运行期真实 _G 的表面。
        lua_api_surface_json: result.host.lua_api_surface_json,
        host_api_module_names_json: result.host.host_api_module_names_json
      };
      wasmState.luaBridge = 'ok';
    } catch (error) {
      wasmState.luaBridge = 'error:' + hostError(error);
    }
    // WP10 批次 A：wasm 导出 HostApi 构造器 → 实例化挂到 window.__RUSTLUA_HOST_API。
    //（Rust 侧 LuaRuntime::register_api 已注册 clock/calendar/day，此桥仅保留为
    //  诊断兼容；web-runtime.js 不再经它注册宿主全局。）
    if (typeof result.host.HostApi === 'function') {
      try {
        window.__RUSTLUA_HOST_API = new result.host.HostApi(WECHAT_PUBLISH_NAME);
        wasmState.hostApi = 'ok';
      } catch (error) {
        wasmState.hostApi = 'error:' + hostError(error);
      }
    } else {
      wasmState.hostApi = 'no-HostApi-export';
    }
  }

  var api = {
    canvas: canvas,
    status: statusElement,
    boot: bootAttributes,
    fetch: fetchShim,
    readText: readText,
    // 宿主自己加载的接口（音频等）要的路径：主体路由 + 方言 + 远端 URL 一起算好。
    // 贴图那条走 patchImageSrc 改写 src，音频没有可改写的对象，只能问这里。
    assetSrcFor: assetSrcFor,
    ensurePack: ensurePack,
    probePackReadability: probePackReadability,
    // 分包形态的形态入口：清单（路由依据）与分包状态。harness 的断言直接读这两个，
    // 不去猜内部变量名。
    loadFirstScreenManifest: loadFirstScreenManifest,
    // 代码包改名查询：无头验证拿它核对"运行时查到的宿主名"与产物里真实存在的文件名一致。
    codePackageInfo: function () {
      return { renaming: renamingInfo(), hostFileFor: hostFileFor };
    },
    firstScreenInfo: function () {
      var manifest = firstScreenKeys();
      return {
        form: manifest ? 'mainpack+pack' : 'none',
        manifest: MAINPACK_MANIFEST,
        entries: manifest ? manifest.count : 0,
        layout: manifest ? manifest.layout : null,
        mainpack_reads: mainpackReads,
        isFirstScreen: isFirstScreenPath
      };
    },
    packInfo: function () {
      return {
        declared: hasPackSubpackage(),
        name: packSubpackageName(),
        attempted: packMountAttempts > 0,
        state: packState,
        progress: packProgress,
        progress_events: packProgressEvents,
        mount_frame: packMountFrame,
        failure: packFailure
      };
    },
    readStrategy: function () { return preferredStrategy.pack; },
    readStrategies: function () { return { main: preferredStrategy.main, pack: preferredStrategy.pack }; },
    // 宿主 FS 路由包装的状态与命中数（无头验证拿它证明"这次启动真的走了路由"，而不是只看门是否变绿）。
    fsRouteInfo: fsRouteInfo,
    // 微信侧 GL 能力（webgl2 / webgl / 上限）：WP8 要在微信上接引擎渲染器，先得知道宿主给不给
    // WebGL2。真机/开发者工具里执行 `__RustLuaWechat.glCapability()` 即可拿到同一份读数。
    glCapability: probeGlCapability,
    // 引擎宿主的 wasm 侧状态**直读**（`engine_host_state` 的 JSON，含 `pixel_sample` 像素回读）。
    // 未 boot / 内核无 GPU 路径都返回具名 JSON 而不是抛异常——验收侧要的是读数，不是栈。
    engineHostState: function () {
      var host = engineHostRef;
      if (!host || typeof host.engine_host_state !== 'function') {
        return JSON.stringify({
          ok: false,
          reason: host ? 'kernel-without-engine-host-state' : 'kernel-not-loaded-yet'
        });
      }
      try {
        return host.engine_host_state();
      } catch (error) {
        return JSON.stringify({ ok: false, reason: String((error && error.message) || error).slice(0, 300) });
      }
    },
    // 渲染自证：开发者工具控制台里可直接 `__RUSTLUA_DIAG()` 复制一份现场快照
    // （主画布来源、像素回读、帧数、贴图成败、远端状态、wasm 核心状态）。
    diag: diagSnapshot,
    publishDiag: publishDiag,
    // wasm 核心：状态查询 + 显式触发（默认在 start() 里触发一次；这里给测试与现场排查用）。
    wasmInfo: function () { return wasmState; },
    loadWasm: loadWasmCoreOnce,
    canvasInfo: function () {
      return { source: canvasSource, isMain: canvasIsMain, isCreated: canvasIsCreated, width: canvas.width, height: canvas.height };
    },
    enableDebugOverlay: function (on) { debugOverlayEnabled = !!on; return debugOverlayEnabled; },
    loadRemoteConfig: loadRemoteConfig,
    remoteStats: function () { return assetStats; },
    // 供测试直接核对 FNV-1a 实现与 assetc 清单一致（不需要真的走一次远端下载）。
    checksumOf: function (bytes) { return fnv1a64Hex(bytes); },
    // 远端缓存的**发布标识**与缓存路径：无头验证用它核对"换一版资源后旧缓存不再被命中"
    // （这条规则只看行为很难测——要真的下载两次；看路径则一眼可判）。
    releaseKey: releaseKey,
    cachePathFor: cachePathFor,
    loadGameFont: loadGameFont,
    showBootError: showBootError,
    start: start,
    version: ADAPTER_VERSION,
    view: { width: VIEW_WIDTH, height: VIEW_HEIGHT },
    fontFamily: function () { return gameFontFamily; },
    getAttribute: function (name) { return bootAttr(name); },
    // 供自动化测试判定启动结果（plugins/wechat-minigame/README.md §验证）
    isBootOk: function () { return bootAttr('data-rustlua-boot') === 'ok'; }
  };
  // 路由包装必须早于**任何**读者：`game.js` 的顺序是 adapter → web-runtime.js → adapter.start()，
  // 而 web-runtime.js 自己在启动期就会直接问 wx 要 `pack/manifest.txt`。在这里装，正好在
  // `start()` 之前、也在模块加载完的同一拍里。
  var fsRouted = installFsRouting();
  g.__RustLuaWechat = api;
  // 开发者工具控制台可直接调用：`__RUSTLUA_DIAG()` / `__RustLuaWechat.canvasInfo()`。
  g.__RUSTLUA_DIAG = diagSnapshot;
  // 控制台的"上下文"与游戏全局可能不是同一个对象（实测 `__RUSTLUA_DIAG is not defined`），
  // 因此把同一批入口也挂到 `wx` 上：只要控制台能看到 wx，就能 `wx.__rustluaDiag()`。
  var wxBridge = 'no-wx';
  try {
    if (typeof wx !== 'undefined' && wx) {
      wx.__rustluaDiag = diagSnapshot;
      wx.__rustluaCanvasInfo = api.canvasInfo;
      wx.__rustluaEngineHostState = api.engineHostState;
      wx.__rustluaVersion = ADAPTER_VERSION;
      wx.__rustluaPublishDiag = publishDiag;
      wxBridge = 'ok';
    }
  } catch (e) { wxBridge = 'fail:' + hostError(e); }
  // 这一行是"现场跑的到底是哪一版适配层"的唯一凭据：代码包会被开发者工具缓存，
  // 光看"有没有报错"分不清旧包新包（实测踩过）。控制台搜 `适配层已就绪` 即可确认版本。
  log('wechat-adapter: 适配层已就绪 v' + ADAPTER_VERSION + '（__RustLuaWechat / __RUSTLUA_DIAG 已注册；wx 通道=' + wxBridge +
    '；宿主 FS 路由=' + (fsRouted ? 'on' : 'off') + '）');
  publishDiag();
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})();


