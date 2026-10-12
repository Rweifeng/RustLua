#!/usr/bin/env node
'use strict';

/**
 * 首屏清单生成器 —— 微信小游戏「主包 + 单分包」切分的唯一事实来源。
 *
 * 背景（微信硬规则，实测核对过官方文档）：
 *   game.json 里 `subpackages[].root` 目录下的**全部**文件都会被打进分包，
 *   主包里一个字节都不能留。所以「主包保留启动场景及其贴图」不能表现为
 *   「在 pack/ 里留一部分」，必须另起一个兄弟目录 mainpack/。
 *   运行时（web-runtime.js）把资源路径硬编码为 `pack/<x>`，且被 webgl / 微信
 *   两个平台共用同一份文件；路由全部落在 wechat-adapter.js 里，它按**本清单**
 *   把 `pack/<x>` 静态路由到 `mainpack/<x>`（清单内）或 `pack/<x>`（清单外）。
 *
 * 因此本清单是构建期（package.ps1 按它搬运文件）、运行时（适配层按它路由）、
 * 验收期（harness / verify 按它断言「清单项在主包且不在分包」）三方共用的契约。
 *
 * 保守性原则：**宁可多列，不可漏列**，但"多列"要列在入口界面上。
 *   漏列 → 首屏要用的资源被路由到分包 → 分包未挂载时读不到 → 贴图 onerror。
 *   多列 → 该资源留在主包、分包变小。**上限卡的是主包那一侧**（微信小游戏：主包 ≤ 4MB、
 *          单个普通分包不单独限大小、总量 ≤ 20MB，口径见 verify 头部注释），
 *          实测也确实是主包紧（examples/game_jhyt 分包形态：主包 3,736 KB / 4,096 KB = 91.2%，
 *          读数只信 verify 的 `main_kb=… main_of_limit=…%`，别用 du 估——本机虚高约 350 KB）。
 *          所以清单的语义必须是"入口界面要用的"，不能顺手扩成"某一类资源的全部"
 *          （见下方 DEFAULT_INCLUDE_PREFIXES 的取舍）。
 *
 * 漏列不会静默：适配层的 assetStats.imageFailed / 屏幕红条 / data-rustlua-boot-detail
 * 三条既有通道会把它变成可见失败，harness 的 imageFailures > 0 断言直接红。
 *
 * 数据来源：以 pack/ 的**实际磁盘文件**为权威，manifest.txt 只用于交叉校验
 * （assetc 不把 manifest.txt / settings.toml 写进清单，只信清单会让骨架掉进分包）。
 *
 * 同时产出**改名映射** `renamed`：微信代码包只上传后缀在白名单内的文件（见
 * scripts/code-package-rules.js），`*.lua` / `*.toml` / `*.md` 传不上去 —— 真机上
 * `readFileSync` 得到的是 `permission denied`，启动直接失败（实测）。因此构建期把这些
 * 文件的文件名追加 `.txt`，运行时按本清单的映射表查回宿主文件名（**只查表、不重算规则**）。
 * 映射覆盖整棵 pack 树（不只首屏），这样 full / remote 形态也是上传合法的。
 *
 * 用法：
 *   node scripts/first-screen-pack.js <packRoot> [--out <mainpack.json>]
 *          [--json] [--print-list] [--include <pack相对前缀>]... [--check <mainpack.json>]
 *   node scripts/first-screen-pack.js --check-whitelist <目录>...
 *
 * 退出码（沿用仓库统一约定）：0 通过 / 1 清单不可靠 / 3 未找到 pack 根。
 */

const fs = require('fs');
const path = require('path');
const rules = require('./code-package-rules.js');

const EXIT_OK = 0;
const EXIT_UNRELIABLE = 1;
const EXIT_NOT_FOUND = 3;

// 场景文件的组件字段分隔符是 `;`，但 `Text.text` 里可能出现 `;` 或 `=`（中文文案、标点）。
// 这里与 web-runtime.js 的 parseComponentFields 保持**逐字一致**的语义，
// 否则工具算出来的闭包会与运行时真正读的贴图漂移。
const COMPONENT_FIELD_PATTERN = /([A-Za-z_][A-Za-z0-9_]*)=/g;

// 保守前缀：只补"静态推不出来、但入口界面一定会画"的那一类。
// 启动场景的贴图闭包（第 5 步）已含 main.scene 里 7 张静态 Image；剩下的是 ui/title_menu.lua
// 按 skin 键动态画的装饰（四角 ornaments、bamboo divider、seal、hover 态）与从未被静态引用到的
// 按下态 pressed —— 键名要过 ui/skin.lua 的 tex 注册表才落成路径，静态推不出来，
// 所以整目录纳入 assets/art/ui/menu/（13 张）。
//
// 为什么不再整收 assets/art/ui/**：其余 10 个子目录（common 24 / hud 19 / skills 6 / backpack 5 /
// dialogue 3 / save 2 / build 2 / settings 2 / shop 2 / quest 2）只被局内面板引用
// （shop.lua / inventory_ui.lua / pause_menu.lua / quest_ui.lua / hud.lua），标题画面一张都不画；
// 它们留在分包 pack/，分包挂载后照常可读（harness 的 subpackage never/delay 两档就是
// "分包还没挂上"的负控，首屏缺图会直接红）。
//
// 收益把话说小（实测 examples/game_jhyt，逐文件 stat 求和，不要用 du）：主包内 art 从
// 69,367 B（82 文件）降到 58,271 B（15 文件），省 11,096 B —— 在 3,729 KB 的主包里占 0.3%，
// 改这里**不是**为了过 4MB 线；为的是"首屏清单 = 入口界面要用的"这个契约本身（此前它等价于"全部 UI"）。
// 需要临时多带某目录时用 --include <pack 相对前缀>，不动这里的默认值。
//
// M59 追加、M71 退役 `assets/ui/prefabs/`：JSON 预设层已整体退役（dialogue.ui.json 删除、
// `ui/prefab.lua` 的 validate 登记移除），目录为空不再首屏携带。运行期 `.prefab`
// （assets/prefabs/*.prefab）由 `on_create` 里 `prefab.spawn` 走 VFS 读，不属于首屏清单。
//
// M71 追加 `assets/art/ui/hud/` 与 `assets/art/ui/skills/`：HUD 是**常驻**界面，其动态层
// （状态图标 hud_status_*、资源条图标 hud_icon_*、头像框、招式卡图标）由 `ui.image` 每帧
// 绘制，静态推不出来也不在 `.prefab` 的贴图闭包里；首屏不带 = PC/小游戏发布后 HUD 图标
// 全部缺失（2026-10-06 PC 实测：招式卡图标空白）。其余局内面板目录（common/backpack/
// shop/quest/settings/save/dialogue/touch）只在面板打开时绘制，仍留在分包。
const DEFAULT_INCLUDE_PREFIXES = [
  'assets/art/ui/menu/',
  'assets/art/ui/hud/',
  'assets/art/ui/skills/'
];

const MANIFEST_HEADER_KEY = 'assetc_manifest';

function log(message) {
  process.stdout.write(message + '\n');
}

function warn(message) {
  process.stderr.write('first_screen_pack_warn ' + message + '\n');
}

// ------------------------------------------------------------------ 解析

// 与 web-runtime.js 的 keyValue 同语义：找 `key =` 开头的行，去首尾引号。
function keyValue(text, key, fallback) {
  for (const raw of String(text || '').split(/\r?\n/)) {
    const line = raw.trim().replace(/^﻿/, '');
    const prefix = key + ' =';
    if (line.startsWith(prefix)) return line.slice(prefix.length).trim().replace(/^"|"$/g, '') || fallback;
  }
  return fallback;
}

// 与 web-runtime.js 的 parseComponentFields 同语义（见上方常量注释）。
function parseComponentFields(raw) {
  const text = String(raw || '');
  const marks = [];
  let match;
  COMPONENT_FIELD_PATTERN.lastIndex = 0;
  while ((match = COMPONENT_FIELD_PATTERN.exec(text)) !== null) {
    const boundary = match.index === 0 ? 0 : (text[match.index - 1] === ';' ? match.index - 1 : -1);
    marks.push({ key: match[1], valueStart: match.index + match[0].length, boundary });
  }
  const out = {};
  for (let i = 0; i < marks.length; i += 1) {
    const end = i + 1 < marks.length && marks[i + 1].boundary >= 0 ? marks[i + 1].boundary : text.length;
    out[marks[i].key] = text.slice(marks[i].valueStart, end);
  }
  return out;
}

// 从场景文本里抽出所有贴图引用。来源有三类字段：
//   Image / Sprite / Tilemap → `texture=<pack相对路径>`
//   Animation               → `clip=<name>:<sheet>:<cols>:<fps>:<looped>`，第 2 段是图集
function sceneTextureRefs(text) {
  const refs = new Set();
  for (const line of String(text || '').split(/\r?\n/)) {
    const parts = line.split('\t');
    if (parts[0] !== 'component') continue;
    const fields = parseComponentFields(parts.slice(3).join('\t'));
    if (fields.texture) {
      const value = fields.texture.trim();
      if (value) refs.add(value);
    }
    if (fields.clip) {
      const segments = fields.clip.split(':');
      const sheet = (segments[1] || '').trim();
      if (sheet) refs.add(sheet);
    }
  }
  return refs;
}

// 引擎内置资源哨兵：以 `__name__` 形式出现，由渲染器**就地合成**，本来就不占资源清单。
//
// 现在只有白纹理：`WHITE_TEXTURE_PATH = "__white__"`（引擎 `render/frame.rs`），纯色块
// （`Image texture=__white__;color=…`，UI 面板/HUD 就这么用）不需要任何贴图文件。
// 以前这里把启动场景里的 `__white__` 当成**悬空引用**直接拒绝发布，于是"场景里只要有一个
// 没贴图的纯色 Quad"就永远发不出微信包 —— `examples/game_api` 实测就是这么卡住的
// （`first_screen_pack_warn 启动场景 scenes/index.scene 引用的贴图不在资源清单里: __white__`）。
// 用双下划线约定而不是只认一个常量：将来同类哨兵（引擎自己画的默认资源）不必再回来改这里。
function isEngineBuiltinRef(ref) {
  return ref === '__white__' || /^__[a-z0-9_]+__$/.test(ref);
}

// ------------------------------------------------------------------ 输入

// 递归扫描 pack/ 的**实际文件**，以磁盘为权威。
// 不能只信 manifest.txt：实测 `settings.toml` 与 `manifest.txt` **自己**都不在清单里
// （assetc 不把这两者写进清单），照 manifest 选文件会让骨架 manifest.txt 掉进分包，
// 而适配层启动自检 probePackReadability 第一个读的就是它。
function scanPack(packRoot) {
  const sizes = new Map();
  const walk = (dir, prefix) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const rel = prefix ? prefix + '/' + entry.name : entry.name;
      if (entry.isDirectory()) walk(path.join(dir, entry.name), rel);
      else if (entry.isFile()) sizes.set(rel, fs.statSync(path.join(dir, entry.name)).size);
    }
  };
  walk(packRoot, '');
  return sizes;
}

// manifest.txt：首行 header，其后每行 `路径\t字节数\tFNV校验和`。仅用于与磁盘交叉校验。
function readManifest(packRoot) {
  const manifestPath = path.join(packRoot, 'manifest.txt');
  const text = fs.readFileSync(manifestPath, 'utf8').replace(/^﻿/, '');
  const sizes = new Map();
  const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
  for (let i = 0; i < lines.length; i += 1) {
    const columns = lines[i].split('\t');
    if (i === 0 && columns[0].startsWith(MANIFEST_HEADER_KEY)) continue;
    const rel = (columns[0] || '').trim();
    if (!rel) continue;
    const size = Number(columns[1]);
    sizes.set(rel, Number.isFinite(size) ? size : 0);
  }
  return sizes;
}

function readTextIfPresent(packRoot, rel) {
  const abs = path.join(packRoot, rel.split('/').join(path.sep));
  if (!fs.existsSync(abs)) return null;
  return fs.readFileSync(abs, 'utf8').replace(/^﻿/, '');
}

// ------------------------------------------------------------------ 闭包

function buildEntries(packRoot, includePrefixes) {
  const catalog = scanPack(packRoot);
  if (catalog.size === 0) throw new Error('pack/ 里没有任何文件: ' + packRoot);

  const warnings = [];
  for (const rel of readManifest(packRoot).keys()) {
    if (!catalog.has(rel)) warnings.push('manifest-listed-missing-on-disk=' + rel);
  }

  const projectToml = readTextIfPresent(packRoot, 'project.toml');
  if (projectToml === null) throw new Error('缺少 pack/project.toml（无法确定启动场景与主脚本）');

  const startupScene = keyValue(projectToml, 'startup_scene', 'scenes/main.scene');
  const mainScript = keyValue(projectToml, 'main_script', 'scripts/main.lua');
  const font = keyValue(projectToml, 'font', '');

  const selected = new Set();
  const add = (rel) => {
    if (rel && catalog.has(rel)) selected.add(rel);
  };

  // 1) 根骨架：manifest 本身 + project.toml 必须主包侧可读（适配层的自检探针就读它们）。
  for (const rel of ['manifest.txt', 'project.toml', 'settings.toml', 'chars.txt']) add(rel);

  // 2) 启动场景与主脚本。
  add(startupScene);
  add(mainScript);

  // 3) scripts/ 整体进主包（不只是 *.lua）。
  //    web-runtime.js 启动时会遍历 manifest，把所有 scripts/**.lua 逐个读进 package.preload，
  //    而 Lua 的 require 是同步的、无法改成异步懒加载 —— 少一个脚本，启动就断。
  //    目录里另有非 .lua 文件（如 README.md）也一并纳入：多列无副作用，漏列才致命。
  const scripts = [];
  for (const rel of catalog.keys()) if (rel.startsWith('scripts/')) scripts.push(rel);
  scripts.forEach(add);

  // 4) 字体必须主包侧：适配层 loadGameFont 在 boot **之前**读它，且 wx.loadFont 只吃本地路径。
  //    但它读的是 project.toml **声明**的那一份（font，以及 font_fallback 若配置）。此前这里
  //    收整个 assets/fonts/ 目录，等于断言"目录里每个字体都是启动期宿主加载"——目录里留着的
  //    停用全量字体会直接压垮主包配额（实测 jhyt：NotoSansSC-VF.ttf 17.7 MB 未声明、未引用，
  //    主包 19,928 KB > 4,096 KB 打不出包）。未声明的字体仍随分包 `pack/` 发布，只是不占首屏。
  //    font 键缺失时保留"全收"的旧口径并逐条警告——适配层会按默认字体加载，静态推不出它对应
  //    哪一份文件，漏列才致命、多列无副作用（与 scripts/ 同一条判据）。
  //    路径单独留一份（fontPaths）：下面要用它断言"宿主自己加载的资源不得被改名"。
  const declaredFonts = [font, keyValue(projectToml, 'font_fallback', '')]
    .filter(Boolean)
    .map((p) => p.replace(/\\/g, '/'));
  const fontPaths = [];
  let fontFiles = 0;
  for (const rel of catalog.keys()) {
    if (!rel.startsWith('assets/fonts/')) continue;
    if (declaredFonts.length > 0 && !declaredFonts.includes(rel)) continue;
    add(rel);
    fontPaths.push(rel);
    fontFiles += 1;
  }
  // 声明字体落在 assets/fonts/ 之外时同样要进首屏（宿主按真实路径读它）。
  for (const rel of declaredFonts) {
    if (!catalog.has(rel) || fontPaths.includes(rel)) continue;
    add(rel);
    fontPaths.push(rel);
    fontFiles += 1;
  }
  for (const rel of catalog.keys()) {
    if (rel.startsWith('assets/fonts/') && !fontPaths.includes(rel)) {
      warnings.push('font_not_first_screen=' + rel);
    }
  }

  // 5) 启动场景的贴图闭包。
  const sceneRefsByScene = new Map();
  const sceneFiles = [];
  for (const rel of catalog.keys()) if (rel.startsWith('scenes/') && rel.endsWith('.scene')) sceneFiles.push(rel);

  for (const rel of sceneFiles) {
    const text = readTextIfPresent(packRoot, rel);
    if (text === null) continue;
    const refs = sceneTextureRefs(text);
    sceneRefsByScene.set(rel, refs);
    for (const ref of refs) {
      if (catalog.has(ref)) continue;
      // 引擎内置哨兵不是文件：它由渲染器合成，清单里没有**才是对的**（见 isEngineBuiltinRef）。
      if (isEngineBuiltinRef(ref)) continue;
      // 首屏引用悬空 = 发出去的包注定缺图，绝不允许。
      if (rel === startupScene) {
        throw new Error(
          '启动场景 ' + rel + ' 引用的贴图不在资源清单里: ' + ref +
          '（首屏注定缺图，先修资源管线再发布）'
        );
      }
      warnings.push('unresolved=' + ref + ' scene=' + rel);
    }
  }

  const startupRefs = sceneRefsByScene.get(startupScene) || new Set();
  let textures = 0;
  for (const ref of startupRefs) {
    if (catalog.has(ref)) {
      add(ref);
      textures += 1;
    }
  }

  // 5b) UI 预设（M63b）：门面的那批 UI 实体从 `scenes/main.scene` 搬进了
  //     `assets/prefabs/*.prefab`，由脚本在 `on_create` 里 `prefab.spawn` 读进来。
  //     于是同时变了两件事：
  //     ① 启动场景不再有 `component` 行 ⇒ 第 5 步的贴图闭包退化成**空集**，
  //        `assets/art/title/`、`assets/art/ui/logo/` 这些不在保守前缀里的图会**静默**
  //        离开首屏（微信首帧没有背景与 logo，而构建期一行警告都没有）；
  //     ② 预设文件本身是启动期同步读的数据文件，与上面 M59 那份 JSON 同理：分包形态下
  //        `wechatReadFile` 不 await `ensurePack`，文件只躺在 `pack/` 里就是白屏。
  //     "哪份预设属于门面"由 Lua 里的 `PRESET_PATH` 决定，静态推不出来 ⇒ 与 skin 键同一处理：
  //     整目录纳入，并把每份预设的 `component` 行算进贴图闭包。`.prefab` 的行格式与 `.scene`
  //     逐字相同（同是 `component\t<id>\t<Type>\t<value>`），所以 `sceneTextureRefs` 直接复用。
  //     引用悬空按**警告**处理而不是硬失败：与门面无关的预设（商店/背包）引用分包里的图是正当的。
  const prefabFiles = [];
  for (const rel of catalog.keys()) {
    if (rel.startsWith('assets/prefabs/') && rel.endsWith('.prefab')) prefabFiles.push(rel);
  }
  const prefabRefs = new Set();
  let prefabTextures = 0;
  for (const rel of prefabFiles) {
    add(rel);
    const text = readTextIfPresent(packRoot, rel);
    if (text === null) continue;
    for (const ref of sceneTextureRefs(text)) {
      if (!catalog.has(ref)) {
        warnings.push('unresolved=' + ref + ' prefab=' + rel);
        continue;
      }
      prefabRefs.add(ref);
      if (!startupRefs.has(ref)) {
        add(ref);
        prefabTextures += 1;
      }
    }
  }

  // 6) 保守前缀：运行时按 ui.* 动态绘制的那部分 UI 静态推不出来，整目录纳入。
  for (const prefix of includePrefixes) {
    for (const rel of catalog.keys()) if (rel.startsWith(prefix)) add(rel);
  }

  // 7) 拒绝项：`game.js`（与分包入口 pack/game.js、主包入口 game.js 三重同名）与任何**被编译**的文件
  //    （`*.js` / `game.json`）——它们上传后 `readFileSync` 读不到（见 code-package-rules），
  //    列进清单等于给首屏埋一个必然失败的读取。带 pack/ 前缀的也拒绝：会造成读串。
  const entries = Array.from(selected).sort();
  for (const rel of entries) {
    if (rel.startsWith('pack/')) {
      throw new Error('拒绝把 ' + rel + ' 列进首屏清单（带 pack/ 前缀，会与分包路径读串）');
    }
    if (rules.isCompiledName(rel)) {
      throw new Error('拒绝把 ' + rel + ' 列进首屏清单（*.js / game.json 会被编译，上传后读不到）');
    }
  }

  let bytes = 0;
  for (const rel of entries) bytes += catalog.get(rel) || 0;

  // 改名映射：**整棵 pack 树**里后缀不在白名单内的文件（`*.lua` / `*.toml` / `*.md` …）。
  // 为什么覆盖全树而不是只覆盖首屏：full / remote 形态下 `scripts/**.lua` 在分包侧的
  // `pack/` 里，同样传不上去；覆盖全树后构建期断言才能统一要求"代码包里没有非白名单后缀"。
  //
  // 幂等守卫：树里出现 `x.lua.txt` 这种"看起来已经改过名"的形状就拒绝二次推导——
  // 对着一份已处理的产物再跑一次本工具（或源工程里恰好有同名文件）时，
  // 二次追加会产出 `x.lua.txt.txt` 这种谁也读不到的名字。
  const catalogNames = Array.from(catalog.keys());
  for (const rel of catalogNames) {
    if (!rules.looksRenamed(rel)) continue;
    throw new Error('命名看起来已经应用过（' + rel + '）：拒绝二次推导，请对**切分前**的 pack/ 运行本工具');
  }

  const renamed = {};
  for (const rel of catalogNames.slice().sort()) {
    const host = rules.hostNameFor(rel);
    if (host !== rel) renamed[rel] = host;
  }
  // 冲突断言：改名后的名字不得与包内任何已有文件名撞车（含大小写不敏感的碰撞）。
  // 静默覆盖会让某个资源在真机上变成另一个文件的内容。
  const collisions = rules.findRenameCollisions(catalogNames);
  if (collisions.length > 0) throw new Error('改名冲突: ' + collisions.join('; '));
  // **宿主自己加载**的接口族（Image.src / wx.loadFont）拿到的是真实文件路径，改名会让它们
  // 指向一个不存在的名字。本工程这些资源都是白名单后缀（png/ttf）不需要改名，但一旦将来
  // assetc 换了格式就会爆——所以在这里把它变成构建期硬失败，而不是等真机贴图全丢。
  const hostLoaded = new Set();
  for (const rel of startupRefs) if (catalog.has(rel)) hostLoaded.add(rel);
  // 预设里的图同样是宿主自己按真实文件名加载的（spawn 出来的 `Image.texture` 走 `Image.src`），
  // 改名会让它读不到 —— 与启动场景同一口径。
  for (const rel of prefabRefs) hostLoaded.add(rel);
  for (const rel of fontPaths) hostLoaded.add(rel);
  for (const rel of Object.keys(renamed)) {
    if (hostLoaded.has(rel)) {
      throw new Error('改名集合与"宿主自己加载"的资源相交: ' + rel +
        '（Image.src / wx.loadFont 需要真实存在的文件名，改名会让它读不到）');
    }
  }

  return {
    startupScene, mainScript, font, entries, bytes, textures, scripts: scripts.length,
    fontFiles, warnings, sceneCount: sceneFiles.length, renamed,
    prefabCount: prefabFiles.length, prefabTextures
  };
}

function buildManifestObject(plan) {
  const renamedKeys = Object.keys(plan.renamed).sort();
  const renamed = {};
  for (const key of renamedKeys) renamed[key] = plan.renamed[key];
  return {
    version: 1,
    generator: 'scripts/first-screen-pack.js',
    layout: { pack_prefix: 'pack/', main_dir: 'mainpack/', subpackage_root: 'pack' },
    startup_scene: plan.startupScene,
    main_script: plan.mainScript,
    font: plan.font,
    entries: plan.entries,
    count: plan.entries.length,
    bytes: plan.bytes,
    // 逻辑路径 → 会随代码包上传的宿主文件名。**只收录需要改名的**；运行时查不到就用逻辑路径。
    renamed,
    renamed_count: renamedKeys.length
  };
}

// ------------------------------------------------------------------ CLI

function parseArgs(argv) {
  const options = { packRoot: null, out: null, renameMap: null, printList: false, json: false, includes: [], check: null, checkWhitelist: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--out') options.out = argv[++i];
    else if (arg === '--rename-map') options.renameMap = argv[++i];
    else if (arg === '--print-list') options.printList = true;
    else if (arg === '--json') options.json = true;
    else if (arg === '--include') options.includes.push(argv[++i]);
    else if (arg === '--check') options.check = argv[++i];
    // `--check-whitelist` 后面可以跟**多个**目录（`--check-whitelist mainpack pack`）。
    // 早先只吃一个：第二个目录会被当成 packRoot 吞掉，于是"两个目录都查过"变成只查了一个，
    // 而输出行还是绿的（假绿，而且正在发布路径上）。
    else if (arg === '--check-whitelist') {
      while (i + 1 < argv.length && !String(argv[i + 1]).startsWith('--')) {
        options.checkWhitelist.push(argv[++i]);
      }
    }
    else if (arg === '-h' || arg === '--help') options.help = true;
    else if (!options.packRoot) options.packRoot = arg;
    else throw new Error('无法识别的参数: ' + arg);
  }
  return options;
}

function usage() {
  log('用法: node scripts/first-screen-pack.js <packRoot> [--out <mainpack.json>]');
  log('        [--rename-map <code-package.json>] [--json] [--print-list]');
  log('        [--include <前缀>]... [--check <mainpack.json>]');
  log('      node scripts/first-screen-pack.js --check-whitelist <目录>...');
}

// 形态无关的改名映射（`code-package.json`，产物根）：三种发布形态都要有它，因为
// 「后缀不在白名单 → 名加 .txt」这条规则对**任何**形态的代码包都成立，而 `mainpack.json`
// 只在分包形态存在（它同时是形态判据，不能塞进别的语义）。
function buildRenameMapObject(plan) {
  const renamed = {};
  for (const key of Object.keys(plan.renamed).sort()) renamed[key] = plan.renamed[key];
  return {
    version: 1,
    generator: 'scripts/first-screen-pack.js',
    rename_suffix: rules.RENAME_SUFFIX,
    rules_verified_on: rules.RULES_VERIFIED_ON,
    renamed,
    count: Object.keys(renamed).length
  };
}

// 代码包后缀白名单检查：列出目录里**传不上真机**的文件（见 scripts/code-package-rules.js）。
// `*.js` 与 `game.json` 会被编译（上传后读不到）但它们是被 require/平台解析的，不算问题，只做备注。
function checkWhitelist(dirs) {
  let failures = 0;
  let scanned = 0;
  for (const rawDir of dirs) {
    const dir = path.resolve(rawDir);
    if (!fs.existsSync(dir)) {
      warn('--check-whitelist 目录不存在: ' + dir);
      failures += 1;
      continue;
    }
    const offenders = rules.listUnuploadable(fs, path, dir);
    scanned += 1;
    for (const relative of offenders) {
      failures += 1;
      warn('非白名单后缀（真机不会上传此文件）: ' + path.join(rawDir, relative) +
        ' → 期望的宿主文件名 ' + rules.hostNameFor(relative));
    }
  }
  if (failures > 0) {
    warn('code_package_whitelist_fail offenders=' + failures);
    return EXIT_UNRELIABLE;
  }
  log('code_package_whitelist_ok dirs=' + scanned +
    ' (rules_verified_on=' + rules.RULES_VERIFIED_ON.date + '/' + rules.RULES_VERIFIED_ON.libVersion + ')');
  return EXIT_OK;
}

function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    warn(String(error.message || error));
    usage();
    return EXIT_UNRELIABLE;
  }
  if (options.help) {
    usage();
    return EXIT_OK;
  }
  // 独立模式：只做白名单检查（不需要 pack 根，产物目录里跑）。
  if (options.checkWhitelist.length > 0) {
    if (options.checkWhitelist.some((dir) => !dir)) { warn('--check-whitelist 需要目录参数'); return EXIT_UNRELIABLE; }
    return checkWhitelist(options.checkWhitelist);
  }

  const packRoot = options.packRoot ? path.resolve(options.packRoot) : null;
  if (!packRoot || !fs.existsSync(packRoot) || !fs.existsSync(path.join(packRoot, 'manifest.txt'))) {
    warn('没有找到 pack 根（需要 <root>/manifest.txt）: ' + (packRoot || '(未提供)'));
    return EXIT_NOT_FOUND;
  }

  const includePrefixes = DEFAULT_INCLUDE_PREFIXES.concat(options.includes.filter(Boolean));
  let plan;
  try {
    plan = buildEntries(packRoot, includePrefixes);
  } catch (error) {
    warn(String(error.message || error));
    return EXIT_UNRELIABLE;
  }

  const manifest = buildManifestObject(plan);
  const payload = JSON.stringify(manifest, null, 2) + '\n';

  if (options.check) {
    const checkPath = path.resolve(options.check);
    if (!fs.existsSync(checkPath)) {
      warn('--check 指定的清单不存在: ' + checkPath);
      return EXIT_UNRELIABLE;
    }
    const existing = fs.readFileSync(checkPath, 'utf8').replace(/^﻿/, '');
    if (existing.trim() !== payload.trim()) {
      warn('清单与 pack/ 的当前内容不一致（手工编辑漂移？）: ' + checkPath);
      return EXIT_UNRELIABLE;
    }
    log('first_screen_pack_check_ok entries=' + manifest.count + ' bytes=' + manifest.bytes);
    return EXIT_OK;
  }

  if (options.out) {
    const outPath = path.resolve(options.out);
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    // 确定性输出：无 BOM、LF、键序固定 —— 同一份 pack/ 必须产出逐字节相同的清单。
    fs.writeFileSync(outPath, payload, 'utf8');
  }
  if (options.renameMap) {
    const mapPath = path.resolve(options.renameMap);
    fs.mkdirSync(path.dirname(mapPath), { recursive: true });
    fs.writeFileSync(mapPath, JSON.stringify(buildRenameMapObject(plan), null, 2) + '\n', 'utf8');
  }

  if (options.json) log(payload.trim());
  if (options.printList) for (const rel of manifest.entries) log(rel);

  log(
    'first_screen_pack_ok entries=' + manifest.count +
    ' bytes=' + manifest.bytes +
    ' startup=' + manifest.startup_scene +
    ' scripts=' + plan.scripts +
    ' textures=' + plan.textures +
    ' fonts=' + plan.fontFiles +
    ' scenes=' + plan.sceneCount +
    ' prefabs=' + plan.prefabCount +
    ' prefab_textures=' + plan.prefabTextures +
    ' renamed=' + manifest.renamed_count +
    ' warnings=' + plan.warnings.length
  );
  for (const message of plan.warnings) warn(message);
  return EXIT_OK;
}

process.exit(main());
