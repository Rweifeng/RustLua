// 微信小游戏代码包的上传规则（scripts/lib/code-package-rules.js）
//
// 这些是**宿主事实**，不是本工程的偏好。三个地方必须用同一套判据，所以收成零依赖的一份：
//   · 构建期：scripts/build/first-screen-pack.js 决定产物里的文件名；
//             scripts/build/package.ps1 在收尾断言"代码包里没有真机传不上去的东西"；
//   · 无头验证：scripts/verify/wechat-minigame-harness.js 模拟"哪些文件在真机上是真的读得到的"
//             ——**这一层以前完全没模拟**，于是"本机全绿、真机启动失败"（见下）；
//   · 运行时**不用本文件**（小游戏里没有 require 仓库文件的能力）：插件只查清单里的
//     `renamed` 映射表，不重算规则，避免构建/运行时两处规则各自漂移。
//
// 两条规则（来源：微信小游戏官方《代码包》文档 + 2026-09-16 真机实测日志）：
//   ① 代码包**只上传后缀在白名单内**的文件。白名单外的文件传不上去（官方文档明写），
//      真机上读它是 `permission denied`。
//   ② `*.js` 与 `game.json` 会被**编译**，上传后无法用 FileSystemManager 直接读
//      （所以适配层**不得**把 game.json 当运行期判据，见 wechat-adapter.js 的 packSubpackageName）。
//
// **已知的观测边界（不要把它当成定论）**：真机日志里 `mainpack/project.toml` 是
// `permission denied`、而 `pack.json` 读得到，但这两条**同时变了两个变量**
// （目录：子目录 vs 根目录；后缀：.toml vs .json），所以它们**不能**证明"障碍只在后缀"。
// 规则 ① 是官方文档事实，因此"白名单外的文件传不上去"**必然成立**——本模块与改名是**必需**的；
// 但它是否**充分**（即 `mainpack/` 里白名单内的文件在真机上是否读得到）仍未证。
// 判据就写在适配层的启动自检那一行里（分包形态下三个探针都路由到 mainpack/）：
//     manifest.txt=ok@mainpack project.toml=FAIL settings.toml=FAIL   → 障碍只在后缀（本模块即充分）
//     manifest.txt=FAIL        …                                       → 嵌套目录也被限制，需要另想办法
// 详见 docs/52 的 R10。
//
// 硬编码白名单可接受，因为**漂移方向是不对称的**：我们抄的清单若偏小，本机多红（安全）；
// 若偏大，本机绿而真机拒（危险）。而微信只会**增加**后缀，所以自然漂移方向落在安全的那一侧。
'use strict';

// 核对基准：这份白名单是照哪个基础库版本核对的（改白名单时一并更新，构建日志会打印它）。
const RULES_VERIFIED_ON = { date: '2026-09-16', libVersion: '2.32.3' };

// 官方白名单（69 个后缀，全部小写；比对时不区分大小写）。
const EXTENSION_WHITELIST = [
  'png', 'jpg', 'jpeg', 'gif', 'svg', 'js', 'json', 'cer', 'obj', 'dae', 'fbx', 'mtl', 'stl',
  '3ds', 'mp3', 'pvr', 'wav', 'plist', 'ttf', 'fnt', 'gz', 'ccz', 'm4a', 'mp4', 'bmp', 'atlas',
  'swf', 'ani', 'part', 'proto', 'bin', 'sk', 'mipmaps', 'txt', 'zip', 'tt', 'map', 'ogg',
  'silk', 'dbbin', 'dbmv', 'etc', 'lmat', 'lm', 'ls', 'lh', 'lani', 'lav', 'lsani', 'ltc',
  'aac', 'astc', 'br', 'csv', 'cur', 'dat', 'dds', 'glb', 'gltf', 'ico', 'ktx', 'lmani', 'lml',
  'pkm', 'prefab', 'scene', 'skel', 'wasm', 'xml'
];

const WHITELIST = new Set(EXTENSION_WHITELIST);

// 需要改名时一律追加这个后缀：末位后缀变成白名单内的 `txt`，原扩展名保留在名字里（可读、可追溯）。
const RENAME_SUFFIX = '.txt';

function basenameOf(filePath) {
  const text = String(filePath == null ? '' : filePath);
  const index = Math.max(text.lastIndexOf('/'), text.lastIndexOf('\\'));
  return index < 0 ? text : text.slice(index + 1);
}

function dirnameOf(filePath) {
  const text = String(filePath == null ? '' : filePath);
  const index = Math.max(text.lastIndexOf('/'), text.lastIndexOf('\\'));
  return index < 0 ? '' : text.slice(0, index + 1);
}

// 取**末位**后缀（`x.lua.txt` → `txt`）。无后缀（如 `LICENSE`）返回空串。
function extensionOf(filePath) {
  const name = basenameOf(filePath);
  const index = name.lastIndexOf('.');
  return index <= 0 ? '' : name.slice(index + 1).toLowerCase();
}

function isWhitelistedExtension(filePath) {
  return WHITELIST.has(extensionOf(filePath));
}

// 会被编译、上传后读不到的文件：所有 `*.js`，以及 `game.json`。
function isCompiledName(filePath) {
  const name = basenameOf(filePath).toLowerCase();
  return name.endsWith('.js') || name === 'game.json';
}

// **上传后能不能被 readFileSync / fetch 读到**。这才是"运行期可用"的判据：
// 白名单内的文件会上传，但被编译的那两类（js / game.json）上传后读不到。
// 注意 `*.js` 读不到**不代表不能用**——`require` 走的是编译产物，那是另一条通道。
function isReadableInPackage(filePath) {
  return isWhitelistedExtension(filePath) && !isCompiledName(filePath);
}

// 逻辑路径 → 会随代码包上传的**宿主文件名**（目录部分不变，只动文件名）。
function hostNameFor(logicalPath) {
  const text = String(logicalPath == null ? '' : logicalPath);
  if (isWhitelistedExtension(text)) return text;
  return dirnameOf(text) + basenameOf(text) + RENAME_SUFFIX;
}

function isRenamedPath(logicalPath) {
  return !isWhitelistedExtension(logicalPath);
}

// "这个名字看起来是**已经改过名**的结果"（`main.lua.txt` / `project.toml.txt`）：
// 末尾是改名后缀，且内层名字**本身带一个非白名单扩展名**。
// 内层必须**有**扩展名这一条不能省：`chars.txt` 去后缀是 `chars`（没有扩展名），
// 它不是改名的产物，而是本来就该叫这个名——早先漏了这一条，构建期把 `chars.txt` 误判成
// "已改过名"并拒绝出包（假阳性，而且挡在发布路径上）。
function looksRenamed(filePath) {
  const text = String(filePath == null ? '' : filePath);
  if (!text.endsWith(RENAME_SUFFIX)) return false;
  const inner = text.slice(0, text.length - RENAME_SUFFIX.length);
  return extensionOf(inner) !== '' && !isWhitelistedExtension(inner);
}

// 遍历一棵树，回调每个文件的**精确大小写**相对路径（`/` 分隔）。
// `fs`/`path` 由调用方传入，保持本模块不直接依赖 Node（便于被不同脚本复用与单测）。
function walkFiles(fs, nodePath, rootDir, onFile) {
  const walk = (dir) => {
    let entries = [];
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch (error) { return; }
    for (const entry of entries) {
      const full = nodePath.join(dir, entry.name);
      if (entry.isDirectory()) { walk(full); continue; }
      if (!entry.isFile()) continue;
      onFile(nodePath.relative(rootDir, full).split(nodePath.sep).join('/'));
    }
  };
  walk(rootDir);
}

// **两个语义分开的函数**（刻意不合成一个返回 `{reason}` 的列表）：
// 调用方极容易写成 `if (problems.length) throw`，而"被编译"里的 `game.js` / `pkg/fengari-web.js`
// 是**必须存在**的（它们由 require / 平台解析），把两者混在一起会当场把每个产物都判死。
//   listUnuploadable = 后缀不在白名单 → 真机压根没上传 → **必须处理**
//   listUnreadable   = 会上传但被编译 → 读不到（但 require 走的是另一条通道，通常不是问题）
function listUnuploadable(fs, nodePath, rootDir) {
  const out = [];
  walkFiles(fs, nodePath, rootDir, (relative) => {
    if (!isWhitelistedExtension(relative)) out.push(relative);
  });
  return out.sort();
}

function listUnreadable(fs, nodePath, rootDir) {
  const out = [];
  walkFiles(fs, nodePath, rootDir, (relative) => {
    if (isWhitelistedExtension(relative) && isCompiledName(relative)) out.push(relative);
  });
  return out.sort();
}

// 同一个目录里，改名后的名字必须与所有文件名**两两不同**（含大小写不敏感的碰撞）：
// `x.lua → x.lua.txt` 撞上已存在的 `x.lua.txt`（或 Windows/macOS 上的 `X.LUA.TXT`）会静默覆盖，
// 让某个资源在真机上变成另一个文件的内容。返回冲突描述数组（空 = 通过）。
function findRenameCollisions(fileNames) {
  const byHost = new Map();
  const original = new Set();
  for (const name of fileNames) original.add(name.toLowerCase());
  for (const name of fileNames) {
    if (!isRenamedPath(name)) continue;
    const host = hostNameFor(name).toLowerCase();
    if (original.has(host)) {
      byHost.set(host, (byHost.get(host) || []).concat('与包内已有文件撞名: ' + name + ' → ' + hostNameFor(name)));
    }
  }
  return Array.from(byHost.values()).sort();
}

module.exports = Object.freeze({
  EXTENSION_WHITELIST: Object.freeze(EXTENSION_WHITELIST),
  RENAME_SUFFIX,
  RULES_VERIFIED_ON: Object.freeze(RULES_VERIFIED_ON),
  extensionOf,
  isWhitelistedExtension,
  isCompiledName,
  isReadableInPackage,
  hostNameFor,
  isRenamedPath,
  looksRenamed,
  walkFiles,
  listUnuploadable,
  listUnreadable,
  findRenameCollisions
});
