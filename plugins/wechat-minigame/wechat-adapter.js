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
//   Lua 桥导出挂到 `window.__P2_LUA_BRIDGE`（`lua_boot` 包装成 `lua_boot_with_reader`），
//   全部 Lua 交互经桥执行——这条路径在小游戏里**真实可运行**；
// - `pkg/game_web.js` + `pkg/game_web_bg.wasm` 随包分发（胶水是 `--target no-modules` 的
//   纯脚本，由 wasm-bridge.js 包成 CommonJS）。适配层在 boot 之前完成实例化，并把结果
//   （用了哪条通道、哪个代码包路径）写进 `data-p2-wasm` / `__P2_DIAG().wasm`，
//   详见文末「WASM 说明」。
// - 输入：微信 PC 端键盘（wx.onKeyDown/onKeyUp）直接桥接；触摸映射为指针事件，
//   并提供左侧虚拟摇杆（WASD）与右侧按键（space）以保证手机端可玩。
'use strict';

(function () {
  var g = typeof globalThis !== 'undefined' ? globalThis : this;
  // 版本号会打进日志和 __P2_DIAG()：开发者工具会缓存代码包，
  // "有没有报错" 分不清旧包新包，只有版本号能分清（实测踩过）。
  var ADAPTER_VERSION = '0.7.1+wasm-path';
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
// wasm 核心在代码包里的逻辑路径（发布脚本按 `pkg/game_web_bg.wasm` 放置；`.wasm` 在微信
// 代码包后缀白名单内，因此**不需要**改名，运行时按逻辑路径读取即可）。
var WASM_MODULE_PATH = 'pkg/game_web_bg.wasm';
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
    { name: 'user-data', apply: function (p) { return userRoot ? userRoot + '/' + p : null; } },
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
  var mainpackReads = 0;

  function loadFirstScreenManifest() {
    if (firstScreenLoaded) return firstScreen;
    firstScreenLoaded = true;
    var text = null;
    try { text = readText(MAINPACK_MANIFEST); } catch (error) { text = null; }
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

  function readText(path) {
    var result = readWithStrategies(path, readTextRaw);
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
      { path: PACK_PREFIX + 'project.toml', shape: looksLikeToml },
      { path: PACK_PREFIX + 'settings.toml', shape: looksLikeToml }
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
    var text = null;
    try { text = readText(REMOTE_CONFIG_FILE); } catch (error) { text = null; }
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

  function cachePathFor(path) {
    return userRoot ? userRoot + '/' + path : null;
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

  // 清单：`相对路径\tsize\tfnv1a(16 位十六进制)`，首行是 assetc_manifest。
  function loadManifestChecksums() {
    if (manifestChecksums || checksumLookupFailed) return manifestChecksums;
    var text = readText(PACK_PREFIX + 'manifest.txt');
    if (!text) { checksumLookupFailed = true; return null; }
    var map = {};
    var lines = text.split('\n');
    for (var i = 0; i < lines.length; i++) {
      var parts = lines[i].replace(/\r$/, '').split('\t');
      if (parts.length < 3) continue;
      var size = parseInt(parts[1], 10);
      if (isNaN(size)) continue;
      map[parts[0]] = { size: size, checksum: parts[2] };
    }
    manifestChecksums = map;
    return map;
  }

  function verifyDownloaded(path, buffer) {
    if (!remoteConfig.verifyChecksum) return true;
    var manifest = loadManifestChecksums();
    if (!manifest) return true; // 清单不可得时不阻断，但下面会记一笔"未校验"
    var entry = manifest[path.slice(PACK_PREFIX.length)];
    if (!entry) return true;
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
    bodyElement.setAttribute('data-p2-asset-package', String(assetStats.package));
    bodyElement.setAttribute('data-p2-asset-cache', String(assetStats.cache));
    bodyElement.setAttribute('data-p2-asset-remote', String(assetStats.remote));
    bodyElement.setAttribute('data-p2-asset-remote-bytes', String(assetStats.remoteBytes));
    bodyElement.setAttribute('data-p2-asset-verified', String(assetStats.verified));
    bodyElement.setAttribute('data-p2-asset-source', assetStats.remote > 0
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
  // 冷却可现场覆盖（`wx.setStorageSync('p2_pack_retry_cooldown_ms', 0)`）：与启动看门狗同理，
  // 否则"失败后能自愈"这条路径没法被自动化验证（等 10 秒的用例进不了质量门）。
  var PACK_RETRY_COOLDOWN_KEY = 'p2_pack_retry_cooldown_ms';

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
  // data-p2-pack=failed + 屏幕红横幅）。理由：调用方既有 `fetchShim`（它要的是"能不能读"）
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
        bodyElement.setAttribute('data-p2-pack', 'failed');
        bodyElement.setAttribute('data-p2-pack-detail', packFailure.slice(0, 200));
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

  var rawGetContext = canvas.getContext;
  canvas.getContext = function (kind) {
    if (kind !== '2d') return rawGetContext ? rawGetContext.call(canvas, kind) : null;
    if (!canvas.__p2Context) {
      var ctx = rawGetContext ? rawGetContext.call(canvas, '2d') : null;
      if (!ctx) return null;
      canvas.__p2Context = wrapContext(ctx);
    }
    return canvas.__p2Context;
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
  var documentShim = {
    readyState: 'complete',
    body: bodyElement,
    documentElement: bodyElement,
    fonts: { add: function () {} },
    addEventListener: function () {},
    removeEventListener: function () {},
    querySelectorAll: function () { return []; },
    querySelector: function () { return null; },
    createElement: function (tag) {
      return { tagName: String(tag), style: {}, setAttribute: function () {}, appendChild: function () {} };
    },
    getElementById: function (id) {
      if (id === 'p2-canvas') return canvasTarget;
      if (id === 'status') return statusElement;
      return null;
    }
  };

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
    var previous = ctx.__p2AdapterDraw;
    ctx.__p2AdapterDraw = true;
    try { return fn(ctx); }
    catch (error) { warn('wechat-adapter: 适配层自绘失败 -> ' + hostError(error)); return undefined; }
    finally { ctx.__p2AdapterDraw = previous === undefined ? false : previous; }
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
  //   1) `__P2_DIAG()`：一份可复制的 JSON 快照（主画布来源、像素回读、帧数、贴图、远端）；
  //   2) 可开启的屏幕覆盖层：四角色块 + 对角线 + 诊断文字，每帧画在游戏画面**之上**。
  //      开启方式（任一）：发布目录里有 `debug-overlay.txt`，或 wx.getStorageSync('p2_debug_overlay')。
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
    var stored = safeCall(wx, 'getStorageSync', 'p2_debug_overlay');
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
      pixel_probe: pixelProbeValue,
      frames: frameCount,
      version: ADAPTER_VERSION,
      boot: bootAttr('data-p2-boot') || 'unknown',
      boot_detail: String(bootAttr('data-p2-boot-detail') || '').slice(0, 120),
      boot_marks: bootMarkTargetName(),
      drawn: bootAttr('data-p2-drawn') || '0',
      ui_elements: bootAttr('data-p2-ui-elements') || '0',
      world: String(bootAttr('data-p2-world') || '').slice(0, 200),
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
      // hostApi：WP10 微信侧接线状态（ok=HostApi 已实例化挂到 __P2_HOST_API）。
      wasm: {
        status: wasmState.status,
        source: wasmState.source,
        channel: wasmState.channel,
        path: wasmState.path,
        bytes: wasmState.bytes,
        exports: wasmState.exports,
        detail: wasmState.detail,
        attempts: wasmState.attempts,
        luaBridge: wasmState.luaBridge,
        hostApi: wasmState.hostApi
      },
      // 画布 vs 工程声明尺寸（横版方案A）：agree=false 就是"版面会溢出画布"。
      view_agreement: viewAgreement,
      read_strategy: preferredStrategy.pack || '(未探明)',
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

  // `data-p2-world` 是运行时每帧自报的世界状态（地图/实体数/落点/主角坐标与当前动画）。
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
      'P2 DIAG 覆盖层（debug-overlay.txt / p2_debug_overlay 已开启）',
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

  // 分包载入失败横幅：整屏错误页会盖掉游戏，而分包失败不该掀翻标题画面——
  // 所以用一条**常驻红条**说明"资源没下下来，进玩法会缺素材"，并给出重试后的状态。
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
    ctx.fillText('分包（pack）加载失败：进玩法场景会缺素材；游戏从主包照常启动', 12, 63);
    ctx.restore && ctx.restore();
  }

  // 每帧回调之后调用：先计数（永远），再按需覆盖层/警示条。
  // 现场导出通道。为什么要三条：开发者工具控制台的"上下文"**未必就是游戏全局**——实测
  // 日志明明打了 `适配层已就绪 v…（__RustLuaWechat / __P2_DIAG 已注册）`，控制台里
  // `__P2_DIAG()` 却报 "is not defined"（`__RustLuaWechat` 同样不可见）。所以除了挂全局，
  // 再加两条**不依赖全局作用域**的通道：
  //   ① `wx.__p2Diag()` / `wx.__p2CanvasInfo()` —— 控制台里只要能看到 wx 就能调用；
  //   ② `wx.getStorageSync('p2_diag')` —— 连控制台都用不上，直接看开发者工具的"存储"面板。
  function publishDiag() {
    var text;
    try { text = JSON.stringify(diagSnapshot()); } catch (e) { return; }
    try {
      if (typeof wx !== 'undefined' && wx && typeof wx.setStorageSync === 'function') wx.setStorageSync('p2_diag', text);
    } catch (e) { /* 存储不可用不影响游戏 */ }
  }

  function afterFrame() {
    frameCount += 1;
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
    // `data-p2-boot=failed` 时**不画进度条/失败条**：那一刻整屏属于 showBootError 的错误页，
    // 叠上去只会互相遮挡，把真正的失败原因糊掉。
    var bootFailed = String(bootAttr('data-p2-boot') || '') === 'failed';
    if (packState === 'loading' && !bootFailed) withAdapterDraw(ctx, drawLoadingBar);
    if (imageFailures > 0) {
      bandFailures = imageFailures;
      withAdapterDraw(ctx, drawFailureBand);
    }
    if (packState === 'failed' && !bootFailed) withAdapterDraw(ctx, drawPackFailure);
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

  // ---------------------------------------------------------------- 输入桥接

  function toCanvasX(clientX) { return clientX / screenWidth * canvas.width; }
  function toCanvasY(clientY) { return clientY / screenHeight * canvas.height; }

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
  function dispatchWindow(event) {
    // 派发目标是**运行时看到的那个 window**（宿主自带 window 时它是宿主对象，不是 globalThis）。
    var target = (typeof windowTarget !== 'undefined' && windowTarget) ? windowTarget : g;
    if (typeof target.dispatchEvent === 'function') target.dispatchEvent(event);
    else if (typeof g.dispatchEvent === 'function') g.dispatchEvent(event);
    var onname = 'on' + event.type;
    if (typeof target[onname] === 'function') { try { target[onname](event); } catch (e) { /* 忽略业务回调异常 */ } }
  }
  function dispatchCanvas(event) {
    if (typeof canvasTarget.dispatchEvent === 'function') canvasTarget.dispatchEvent(event);
  }

  // ---------------------------------------------------------------- 生命周期与帧循环

  function installLifecycle() {
    function on(name, handler) {
      if (typeof wx[name] === 'function') { try { wx[name](handler); } catch (e) { warn('wechat-adapter: ' + name + ' 注册失败 ' + e); } }
    }
    on('onHide', function () { dispatchWindow(makeEvent('blur', {})); });
    on('onShow', function () { dispatchWindow(makeEvent('focus', {})); });
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
      // 标记要落在运行时读的那个对象上（web-runtime.js 读的是 bare `window`）。
      setWindowFlag('__P2_FONT', fontPath);
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
  // 放宽的后果很隐蔽：宿主那个半成品 body 会让 `data-p2-boot`/`data-p2-drawn` 全都写不进去，
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

  // 每个 shim 全局的安装结果（接替/补齐/补不动/仍缺哪些），进 __P2_DIAG() 供现场核对。
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
  // （实测过）。改成：console.error 喊出来 + 记进 `__P2_DIAG()`，真正的失败交给 `start()`
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

  // `window` 必须带事件通道（运行时把 p2RunLuaGame 挂在它上面，键盘/鼠标事件也从
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

  function patchImageSrc(image) {
    if (!image) return image;
    // 计数与 src 接管互不依赖：先装计数，任何一条路径都不会漏掉"贴图到底成没成"。
    installImageCounters(image);
    var writer = resolveHostSrcWriter(image);
    if (!writer) {
      // 接管不了的两种后果完全不同：包内完整的包照常能加载（宿主自己读 pack/），
      // 需要改写 src 的形态则必然静默丢图甚至卡在启动上——那种情况必须响亮失败（README §已知边界）。
      if (requiresSrcRewrite()) {
        throw new Error(srcRewriteErrorMessage('无法把 Image.src 交给宿主（own/原型访问器与 setSrc 都没有）'));
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
        throw new Error(srcRewriteErrorMessage('宿主不允许改写 Image.src（' + hostError(error) + '）'));
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
      try { wx.setStorageSync(String(key), String(value)); }
      catch (e) { warn('wechat-adapter: 存档写入失败 ' + e); }
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
  // 超时可现场覆盖（`wx.setStorageSync('p2_boot_timeout_ms', 300)`）：慢 CDN 上可以放大，
  // 自动化用例里可以缩短——否则看门狗本身没法被验证（等 10 秒的用例进不了质量门）。
  var BOOT_TIMEOUT_KEY = 'p2_boot_timeout_ms';
  var bootStartedAt = 0;
  var bootWatchdog = null;

  function bootTimeoutMs() {
    var value = parseInt(safeCall(wx, 'getStorageSync', BOOT_TIMEOUT_KEY), 10);
    return value > 0 ? value : BOOT_TIMEOUT_MS;
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
    var message = '启动超时 ' + elapsed + 's：Lua 主循环没有起来。' + stallHint();
    console.error('[wechat-adapter] ' + message + ' 现场：' + JSON.stringify(diagSnapshot()));
    publishDiag();
    showBootError(message);
  }

  var started = false;
  function start(boot) {
    if (started) return Promise.resolve(false);
    started = true;
    bodyElement.setAttribute('data-p2-adapter', 'ready');
    // 必须在 ensurePack 之前读：它决定"分包加载失败是否致命"。
    loadRemoteConfig();
    updateAssetMarkers();
    // 覆盖层开关：发布目录放 debug-overlay.txt，或 wx.setStorageSync('p2_debug_overlay', '1')。
    // 放在这里（而不是模块加载时）是因为它要读文件/存储，且读法依赖已探明的路径方言。
    try {
      debugOverlayEnabled = debugOverlayRequested();
    } catch (error) {
      debugOverlayEnabled = false;
      warn('wechat-adapter: 覆盖层开关读取失败 -> ' + hostError(error));
    }
    if (debugOverlayEnabled) {
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
      bodyElement.setAttribute('data-p2-layout', 'mainpack+pack');
      bodyElement.setAttribute('data-p2-mainpack-entries', String(firstScreen.count));
    }
    // 资源读取自检：分包形态下这三个探针在 mainpack/ 下，验证的正是**路由层本身**。
    probePackReadability();
    // wasm 核心：主包里的 `pkg/game_web_bg.wasm`（属首屏骨架，不在分包里）。
    // Lua 执行层就在这个 wasm 里（WP10 迁移后无备用解释器）：加载失败时宿主交互的
    // 诊断通道（`data-p2-wasm` / `__P2_DIAG().wasm`）会如实记录，而 `__P2_LUA_BRIDGE`
    // 缺失会让 web-runtime.js 抛 `wasm lua bridge not loaded`（发布形态问题，不降级）。
    // **它返回 Promise**：小游戏宿主的 `WXWebAssembly.instantiate(代码包路径, imports)` 只有
    // 异步形态，所以这里只发起加载、把它排在 boot 之前完成（见下面的 wasmReady 链）。
    var wasmReady = loadWasmCoreOnce();
    // 画布与游戏版面的自检：`game.json` 的方向与画布尺寸必须和 `ui/layout.lua` 的设计分辨率
    // 对得上。**为什么值得单独自检**：Lua 面板画在屏幕像素空间、不做缩放，画布尺寸不对的表现是
    // "面板溢出画布"（边缘控件看不见），而这在无头验证里**不会报错**
    // （假 ctx 不裁剪、超界坐标照样计数）→ 只能靠这一行把它变成可读的证据。
    reportViewAgreement();
    // 分包**后台**载入，**不 await**：首屏资源全在主包里，boot 必须立刻用它们把标题画出来。
    // 分包只影响"进玩法场景"那一步（那时的 pack/ 读取会 await 它，见 fetchShim / Image.src）。
    // 失败不能掀翻启动 Promise（见 ensurePack），否则标题已经在屏上却要被整屏错误页盖掉。
    ensurePack().then(function (ok) {      bodyElement.setAttribute('data-p2-pack', ok ? 'ready' : packState);
      if (ok) {
        bodyElement.setAttribute('data-p2-pack-mount-frame', String(packMountFrame));
        bodyElement.setAttribute('data-p2-pack-progress-events', String(packProgressEvents));
        log('wechat-adapter: 分包就绪，首屏之外资源可从 ' + (packSubpackageName() || PACK_SUBPACKAGE) + '/ 读取');
      }
    }).catch(function (error) {
      // ensurePack 自身不 reject；这里只兜住"上面的 then 里抛了"这种意外，
      // 绝不让它变成 unhandled rejection（那在小游戏里表现为静默）。
      warn('wechat-adapter: 分包后台载入异常 -> ' + hostError(error));
    });
    return wasmReady
      .then(function () {
        // wasm（= Lua 执行层）挂载完成之后才允许 boot：`window.__P2_LUA_BRIDGE` 必须在
        // web-runtime.js 走到 Lua 之前存在，否则报的是"桥缺失"这种发布形态错误。
        return loadGameFont();
      })
      .then(function () {
        bootStartedAt = Date.now();
        bootWatchdog = setTimeout(reportBootStall, bootTimeoutMs());
        return boot(statusElement);
      })
      .then(function () {
        if (bootWatchdog) { clearTimeout(bootWatchdog); bootWatchdog = null; }
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
        // 去控制台敲 `__P2_DIAG().wasm` 才拿得到判据。真机没控制台，屏幕就是唯一通道。
        if (wasmState.status !== 'ok') {
          message = message + ' [wasm=' + wasmState.status
            + (wasmState.source ? '/' + wasmState.source : '')
            + (wasmState.detail ? ' detail=' + String(wasmState.detail).slice(0, 200) : '')
            + (wasmState.attempts && wasmState.attempts.length
              ? ' tried=' + String(summarizeAttempts(wasmState.attempts)).slice(0, 220) : '') + ']';
        }
        console.error('[wechat-adapter] 启动失败:', message);
        bodyElement.setAttribute('data-p2-boot', 'failed');
        bodyElement.setAttribute('data-p2-boot-detail', message.slice(0, 400));
        showBootError('启动失败: ' + message);
        drawStartupProbe();
        throw error;
      });
  }

  // WASM 说明（WP10 迁移后）：代码包里发行 `pkg/game_web.js` + `pkg/game_web_bg.wasm`。
  // 胶水用 **`--target no-modules`** 生成（纯脚本、无 import/export），由 `wasm-bridge.js`
  // 包成 CommonJS。**实例化只能走宿主的代码包路径通道**：微信的 `WXWebAssembly` 只暴露
  // `instantiate(path, imports) -> Promise`，第一个参数**只接受代码包路径字符串**
  // （实测开发者工具对字节数组报 `WebAssembly.instantiate: first argument must be a string`；
  // 文档也未提供 Module/Instance 构造器）。字节 `initSync` 那条只在标准 WebAssembly 宿主
  // （本机 Node）成立，保留为兜底通道，且诊断里 `channel` 会写明实际用了哪条。
  // **不复用 webgl 那份 `--target web` 的 ESM 胶水**：小游戏按 CommonJS 解析，
  // 顶层 `export` 会让启动直接 SyntaxError。
  //
  // 当前边界（不得夸大）：wasm 即 Lua 执行层（mlua 5.4 + vendored），`__P2_LUA_BRIDGE`
  // 承载全部 Lua 交互；宿主交互诊断通道（`data-p2-wasm` / `__P2_DIAG().wasm`）如实记录。
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
    bodyElement.setAttribute('data-p2-view', agree === true ? 'ok' : (agree === false ? 'mismatch' : 'unknown'));
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
  // 挂到 `window.__P2_LUA_BRIDGE`（`lua_boot` 包装成 `lua_boot_with_reader`）。
  // 为什么值得做：如果没人验证它"真的装进来了"，Lua 启动失败会被当成"桥没挂上"的假象；
  // 所以这里把结果做成可核对的证据（桥缺失时 web-runtime.js 会抛硬错误，不是静默降级）。
  //
  // 结果档位（都进 `__P2_DIAG().wasm` 与启动日志）：
  //   ok                        实例化成功，`channel` 标出用的是哪条通道，`exports` 是真实导出名清单
  //   no-webassembly            宿主没有 WXWebAssembly / WebAssembly（真机极旧基础库）
  //   webassembly-api-incomplete 宿主既没有 instantiate(path,imports) 也没有 Module/Instance 构造器
  //   path-instantiate-failed    路径通道（小游戏的唯一公开形态）失败，attempts 里带每条候选路径的原因
  //   glue-missing / glue-shape-unexpected  胶水没打进包或形态不对（发布形态问题）
  //   bridge-missing            wasm-bridge.js 自身 require 失败
  //
  // **为什么是 Promise**：`WXWebAssembly` 只有异步的 `instantiate(路径, imports)`（文档如此，
  // 且字节数组会被直接拒），所以 wasm 挂载天生是异步的；boot 必须排在它之后
  // （见 start() 里的 loadGameFont 之前那一步）。
  var wasmState = { status: 'not-attempted' };
  var wasmLoadPromise = null;
  // 画布/版面一致性自检结果（见 reportViewAgreement），进 __P2_DIAG().view_agreement。
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
      bridge.loadWasmCore({
        readBytes: readBytes,
        wasmPath: WASM_MODULE_PATH,
        log: log
      }).then(function (result) {
        if (result.ok) {
          // 顺序要紧：attachWasmBridge 把 luaBridge/hostApi 写在 wasmState 上，
          // 必须等 finish 换好对象之后再挂，否则诊断里这两个字段会被丢掉。
          finish({
            status: 'ok', source: result.source, channel: result.channel, path: result.path,
            exports: result.exports, bytes: result.bytes
          });
          attachWasmBridge(result);
        } else {
          finish({ status: result.reason, source: result.source, detail: result.detail, attempts: result.attempts });
          warn('wechat-adapter: wasm 核心未就绪（' + result.reason
            + (result.detail ? '：' + result.detail : '')
            + (result.attempts ? '｜通道尝试: ' + summarizeAttempts(result.attempts) : '') + '）'
            + '——Lua 执行层在 wasm 里（WP10 迁移后无备用解释器），`__P2_LUA_BRIDGE` 缺失会让'
            + ' web-runtime.js 抛硬错误（发布形态问题，不降级）；"包里带了 wasm 却没装上"正是这种问题');
        }
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
    bodyElement.setAttribute('data-p2-wasm', state.status);
    return wasmState;
  }

  function attachWasmBridge(result) {
    // WP10 迁移（fengari → mlua-wasm）：wasm 导出 Lua 桥函数 → 挂到
    // window.__P2_LUA_BRIDGE，web-runtime.js 的 Lua 交互全部经它完成。
    // 微信没有 wasm 预加载资源表（webgl 的 start() 才有），因此 `lua_boot`
    // 包装成 `lua_boot_with_reader(同步读取器)`：适配层 readText 即该读取器
    //（pack/ 明文，vfs provider 原样交给引擎 pack::decode）。
    // 导出缺失时保持"未就绪"状态（boot 会如实报错），不参与 success 判定。
    if (!result.host) { wasmState.luaBridge = 'no-glue-export'; wasmState.hostApi = 'no-glue-export'; return; }
    try {
      // wasm 侧 vfs 的路径形如 `/project/scripts/main.lua`；适配层 readText 只认
      // `pack/` 前缀的逻辑路径（readSubjects 据此路由到 mainpack/ 首屏清单 + 改名表
      // hostFileFor）。包装器负责转换：剥 `/project/` → 补 `pack/` 前缀。
      function wechatReadFile(path) {
        var rel = String(path).replace(/^\/project\//, '');
        if (rel.indexOf('pack/') !== 0) rel = 'pack/' + rel;
        return readText(rel);
      }
      window.__P2_LUA_BRIDGE = {
        lua_boot: function () { return result.host.lua_boot_with_reader(wechatReadFile); },
        lua_do_string: result.host.lua_do_string,
        lua_call_global: result.host.lua_call_global,
        lua_register_js_callback: result.host.lua_register_js_callback,
        lua_missing_globals: result.host.lua_missing_globals,
        lua_execute_main: result.host.lua_execute_main
      };
      wasmState.luaBridge = 'ok';
    } catch (error) {
      wasmState.luaBridge = 'error:' + hostError(error);
    }
    // WP10 批次 A：wasm 导出 HostApi 构造器 → 实例化挂到 window.__P2_HOST_API。
    //（Rust 侧 LuaRuntime::register_api 已注册 clock/calendar/day，此桥仅保留为
    //  诊断兼容；web-runtime.js 不再经它注册宿主全局。）
    if (typeof result.host.HostApi === 'function') {
      try {
        window.__P2_HOST_API = new result.host.HostApi(WECHAT_PUBLISH_NAME);
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
    // 渲染自证：开发者工具控制台里可直接 `__P2_DIAG()` 复制一份现场快照
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
    loadGameFont: loadGameFont,
    showBootError: showBootError,
    start: start,
    version: ADAPTER_VERSION,
    view: { width: VIEW_WIDTH, height: VIEW_HEIGHT },
    fontFamily: function () { return gameFontFamily; },
    getAttribute: function (name) { return bootAttr(name); },
    // 供自动化测试判定启动结果（plugins/wechat-minigame/README.md §验证）
    isBootOk: function () { return bootAttr('data-p2-boot') === 'ok'; }
  };
  g.__RustLuaWechat = api;
  // 开发者工具控制台可直接调用：`__P2_DIAG()` / `__RustLuaWechat.canvasInfo()`。
  g.__P2_DIAG = diagSnapshot;
  // 控制台的"上下文"与游戏全局可能不是同一个对象（实测 `__P2_DIAG is not defined`），
  // 因此把同一批入口也挂到 `wx` 上：只要控制台能看到 wx，就能 `wx.__p2Diag()`。
  var wxBridge = 'no-wx';
  try {
    if (typeof wx !== 'undefined' && wx) {
      wx.__p2Diag = diagSnapshot;
      wx.__p2CanvasInfo = api.canvasInfo;
      wx.__p2Version = ADAPTER_VERSION;
      wx.__p2PublishDiag = publishDiag;
      wxBridge = 'ok';
    }
  } catch (e) { wxBridge = 'fail:' + hostError(e); }
  // 这一行是"现场跑的到底是哪一版适配层"的唯一凭据：代码包会被开发者工具缓存，
  // 光看"有没有报错"分不清旧包新包（实测踩过）。控制台搜 `适配层已就绪` 即可确认版本。
  log('wechat-adapter: 适配层已就绪 v' + ADAPTER_VERSION + '（__RustLuaWechat / __P2_DIAG 已注册；wx 通道=' + wxBridge + '）');
  publishDiag();
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})();
