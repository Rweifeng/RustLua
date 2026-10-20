// 微信适配层的**多分包根路由表**（docs/67 §5.4）：由 `bundle_plan.json` 推出
// 「路径 → 包根」与「要挂哪些根」，与发布侧 `bundle-layout.js` 共用同一套根命名。
//
// 为什么单独一个模块：适配层（wechat-adapter.js）只该负责 IO/生命周期，
// 路由规则必须能被 Node 自测钉住（不需要微信开发者工具）。
//
// 约定（与 `distribution_layout` / `bundle-layout.js` 一致）：
//   unit=main     → pack（主包，随包存在，不需要 loadSubpackage）
//   unit=sub      → pack-sub
//   unit=sub:<n>  → pack-<n>
//   unit=cdn      → null（发布器另行放置，不参与包内挂载）
//
// 用法：node bundle-roots.js --self-test | --plan <bundle_plan.json> [--paths a,b] [--json]

'use strict';

// **不要在顶层 require('fs')**：这个模块会被微信适配层（wechat-adapter.js）直接 require，
// 小游戏宿主没有 Node 的 fs 模块。CLI 入口在 main() 里再取。
const ROOT_MAIN = 'pack';
const ROOT_SUB = 'pack-sub';

/** 单元 → 包根；`null` 表示不参与包内挂载（cdn）。 */
function rootOfUnit(unit) {
  if (unit === 'main') return ROOT_MAIN;
  if (unit === 'sub') return ROOT_SUB;
  if (typeof unit === 'string' && /^sub:[A-Za-z0-9_-]{1,32}$/.test(unit)) {
    return 'pack-' + unit.slice(4);
  }
  if (unit === 'cdn') return null;
  throw new Error('不认识的 unit: ' + unit);
}

/**
 * 计划表的**结构**校验（口径与 Rust `BundlePlan::parse` + `check_plan` ⑤ 一致）。
 *
 * 为什么在上传产物里再查一遍：适配层是唯一一处"逻辑路径 → 物理根"的解释器，
 * 计划被截断/被手改/打包工具换代都会先在这里表现为**静默路由错**（读不到 = 白图），
 * 而不是报错。这里只查**无歧义**的结构；`checksum` 属于构建期断言，权威实现在
 * Rust（`BundlePlan::parse` 与 `assetc closure`），不在这里重复实现第二份哈希——
 * 两份实现漂移过一次，代价是真机启动失败。
 */
function validatePlanStructure(plan) {
  if (plan.schema !== 1) throw new Error('bundle_plan schema 不支持: ' + plan.schema);
  if (!Number.isInteger(plan.block_size) || plan.block_size <= 0) {
    throw new Error('bundle_plan block_size 必须是正整数');
  }
  var seenNames = Object.create(null);
  var seenPaths = Object.create(null);
  for (var i = 0; i < plan.groups.length; i++) {
    var group = plan.groups[i];
    if (!group || typeof group.name !== 'string' || !group.name) throw new Error('group 缺 name');
    if (seenNames[group.name]) throw new Error('group 名称重复: ' + group.name);
    seenNames[group.name] = true;
    if (typeof group.unit !== 'string') throw new Error('group ' + group.name + ' 缺 unit');
    rootOfUnit(group.unit);
    if (typeof group.bundle !== 'string' || group.bundle.indexOf('bundles/') !== 0) {
      throw new Error('group ' + group.name + ' 的 bundle 路径非法: ' + group.bundle);
    }
    if (!Array.isArray(group.paths)) throw new Error('group ' + group.name + ' 缺 paths');
    var previous = '';
    for (var j = 0; j < group.paths.length; j++) {
      var resource = group.paths[j];
      if (typeof resource !== 'string' || !resource || resource.indexOf('\\') >= 0 ||
          resource.charAt(0) === '/' || resource === '..' ||
          resource.indexOf('../') === 0 || resource.indexOf('/..') >= 0) {
        throw new Error('group ' + group.name + ' 的资源路径非法: ' + resource);
      }
      if (j > 0 && resource < previous) throw new Error('group ' + group.name + ' 的 paths 未排序');
      if (seenPaths[resource]) throw new Error('路径同时属于多个组: ' + resource);
      seenPaths[resource] = group.name;
      previous = resource;
    }
  }
}

/** 解析 `bundle_plan.json`（容忍 BOM），校验必备字段。 */
function parsePlan(raw) {
  const text = String(raw).replace(/^\uFEFF/, '');
  let plan;
  try {
    plan = JSON.parse(text);
  } catch (error) {
    throw new Error('bundle_plan 不是合法 JSON: ' + error.message);
  }
  if (!plan || typeof plan.format !== 'string' || !plan.format.startsWith('assetc-bundle-plan-')) {
    throw new Error('bundle_plan 的 format 不认: ' + (plan && plan.format));
  }
  if (!Array.isArray(plan.groups) || plan.groups.length === 0) {
    throw new Error('bundle_plan 没有 groups');
  }
  validatePlanStructure(plan);
  return plan;
}

/**
 * 计划 → 路由索引：
 *   roots: { [root]: { unit, bundles: string[], paths: string[] } }
 *   pathRoot: { [path]: root | null }
 * 同一路径出现在多个组（计划自校验本应拒绝）→ 这里也响亮失败。
 */
function planRootIndex(plan) {
  const roots = {};
  const pathRoot = {};
  for (const group of plan.groups) {
    if (!group || typeof group.name !== 'string' || typeof group.unit !== 'string') {
      throw new Error('group 缺 name/unit');
    }
    if (!Array.isArray(group.paths)) throw new Error('group ' + group.name + ' 缺 paths');
    const root = rootOfUnit(group.unit);
    if (root === null) continue; // cdn：不参与挂载
    // 不用 `||=`（ES2021）：这个文件会进微信主包，上传器的解析上限是 2019。
    let entry = roots[root];
    if (!entry) {
      entry = { unit: group.unit, bundles: [], paths: [] };
      roots[root] = entry;
    }
    if (typeof group.bundle === 'string' && group.bundle) entry.bundles.push(group.bundle);
    // **容器路径本身也要进路由表**：引擎 `mount_from_reader` 读的是 `pack/bundles/*.rlb`，
    // 而 `group.paths` 只列资源路径、不列容器。少了这一条，具名根的容器会被当成"计划外"
    // 回落到 `pack/`，读不到 → boot 报 `cannot read bundle`。
    if (typeof group.bundle === 'string' && group.bundle) {
      if (Object.prototype.hasOwnProperty.call(pathRoot, group.bundle) &&
          pathRoot[group.bundle] !== root) {
        throw new Error('容器路径同时属于多个根: ' + group.bundle);
      }
      pathRoot[group.bundle] = root;
    }
    for (const path of group.paths) {
      if (Object.prototype.hasOwnProperty.call(pathRoot, path)) {
        throw new Error('路径同时属于多个组: ' + path);
      }
      pathRoot[path] = root;
    }
  }
  for (const key of Object.keys(roots)) roots[key].bundles.sort();
  // wasm 不是 assetc 的资源组，但发布计划表会写明它落在哪个根（默认 pack-wasm）。
  // 把它作为一条**虚拟路径**进路由表：适配层 `wasmLocation` 只问 `rootForPath('game_web_bg.wasm')`。
  const wasmRoot = plan.wasm_root;
  if (typeof wasmRoot === 'string' && wasmRoot) {
    if (!roots[wasmRoot]) roots[wasmRoot] = { unit: 'sub:wasm', bundles: [], paths: [] };
    if (Object.prototype.hasOwnProperty.call(pathRoot, 'game_web_bg.wasm') &&
        pathRoot['game_web_bg.wasm'] !== wasmRoot) {
      throw new Error('wasm_root 与资源路径冲突: ' + wasmRoot);
    }
    pathRoot['game_web_bg.wasm'] = wasmRoot;
  }
  return { roots, pathRoot };
}

/** 一组路径需要哪些**非主包**根（主包随包存在，不需要 loadSubpackage）。 */
function rootsForPaths(index, paths) {
  const needed = new Set();
  for (const path of paths) {
    const root = index.pathRoot[path];
    if (root === undefined) throw new Error('计划里没有这个路径: ' + path);
    if (root !== ROOT_MAIN) needed.add(root);
  }
  return Array.from(needed).sort();
}

/**
 * 单个计划路径 → 包根；`null` 表示计划里没有它。
 * 与 `rootsForPaths` 的区别：适配层读一个路径时不该因为"计划没列"就炸掉整条读取链
 * （根目录文件、运行时生成文件都可能不在计划里），由调用方决定统计/回退。
 */
function rootForPath(index, path) {
  const root = index.pathRoot[path];
  return root === undefined ? null : root;
}

/** 判定一个包根是不是主包（主包随包存在，永远不需要挂载）。 */
function isMainRoot(root) {
  return root === ROOT_MAIN;
}

/** 尚未挂载的根（boot 前用来决定要拉哪些分包）。 */
function missingRoots(index, mounted) {
  const have = new Set(mounted || []);
  return Object.keys(index.roots)
    .filter((root) => root !== ROOT_MAIN && !have.has(root))
    .sort();
}

/**
 * 按顺序挂载给定的根（幂等：已挂载的跳过；主包不需要挂）。
 * `load(root)` 由适配层提供（真实实现是 `wx.loadSubpackage`），失败直接抛出——
 * 调用方决定"响亮失败"还是降级，本函数不做静默吞错。
 * 返回 `{ mounted: 已挂载根（排序）, mountedNow: 本次新挂的根 }`。
 *
 * 刻意不用 async/await：这个模块会进微信主包，仓库里其余发布期 JS 都按 ES5 风格写，
 * Promise 链足够表达"顺序 + 失败传播"，也少一档宿主语法兼容风险。
 */
function mountRoots(index, roots, opts) {
  const load = opts && opts.load;
  if (typeof load !== 'function') return Promise.reject(new Error('mountRoots 需要 load(root)'));
  const mounted = new Set((opts && opts.mounted) || []);
  const mountedNow = [];
  let chain = Promise.resolve();
  roots.forEach((root) => {
    chain = chain.then(() => {
      if (root === ROOT_MAIN || mounted.has(root)) return null;
      if (!index.roots[root]) throw new Error('计划里没有这个根: ' + root);
      return Promise.resolve(load(root)).then(() => {
        mounted.add(root);
        mountedNow.push(root);
      });
    });
  });
  return chain.then(() => ({ mounted: Array.from(mounted).sort(), mountedNow }));
}

/** 读一组路径前，把这些路径所在的非主包根**按顺序**挂好。 */
function ensurePathsMounted(index, paths, opts) {
  return mountRoots(index, rootsForPaths(index, paths), opts);
}

function selfTest() {
  const fixture = {
    schema: 1,
    format: 'assetc-bundle-plan-1',
    block_size: 262144,
    groups: [
      { name: 'core', unit: 'main', bundle: 'bundles/core.rlb', entries: 2, raw_bytes: 1, bundle_bytes: 1, hash: '', paths: ['project.toml', 'scripts/main.lua'] },
      { name: 'assets', unit: 'sub', bundle: 'bundles/assets.rlb', entries: 1, raw_bytes: 1, bundle_bytes: 1, hash: '', paths: ['assets/art/a.png'] },
      { name: 'audio', unit: 'sub:audio', bundle: 'bundles/audio.rlb', entries: 1, raw_bytes: 1, bundle_bytes: 1, hash: '', paths: ['assets/audio/bgm.mp3'] },
      { name: 'brand', unit: 'cdn', bundle: 'bundles/brand.rlb', entries: 1, raw_bytes: 1, bundle_bytes: 1, hash: '', paths: ['splash/splash.png'] },
    ],
    checksum: '',
  };
  const plan = parsePlan(JSON.stringify(fixture));

  // 结构校验（与 Rust `check_plan` ⑤ 同口径）：坏的、会静默路由错的计划表必须响亮失败。
  const mustReject = (label, mutate) => {
    const bad = JSON.parse(JSON.stringify(fixture));
    mutate(bad);
    let threw = false;
    try {
      parsePlan(JSON.stringify(bad));
    } catch (error) {
      threw = true;
    }
    if (!threw) throw new Error('坏的计划表必须被拒绝：' + label);
  };
  mustReject('反斜杠路径', (it) => { it.groups[1].paths[0] = 'assets\\art\\a.png'; });
  mustReject('逃逸路径', (it) => { it.groups[1].paths[0] = '../a.png'; });
  mustReject('绝对路径', (it) => { it.groups[1].paths[0] = '/a.png'; });
  mustReject('未排序', (it) => { it.groups[0].paths.reverse(); });
  mustReject('同名组', (it) => { it.groups[1].name = 'core'; });
  mustReject('跨组重复路径', (it) => { it.groups[1].paths[0] = 'assets/audio/bgm.mp3'; });
  mustReject('未知 unit', (it) => { it.groups[1].unit = 'bundle'; });
  mustReject('容器不在 bundles/ 下', (it) => { it.groups[1].bundle = 'assets.rlb'; });
  mustReject('schema 不认识', (it) => { it.schema = 2; });

  const index = planRootIndex(plan);
  if (index.pathRoot['scripts/main.lua'] !== 'pack') throw new Error('main 路径应路由到 mainpack');
  if (index.pathRoot['assets/audio/bgm.mp3'] !== 'pack-audio') throw new Error('具名分包路径路由错');
  if (Object.prototype.hasOwnProperty.call(index.pathRoot, 'splash/splash.png')) {
    throw new Error('cdn 路径不该进入挂载路由');
  }
  const needed = rootsForPaths(index, ['assets/audio/bgm.mp3', 'scripts/main.lua']);
  if (needed.length !== 1 || needed[0] !== 'pack-audio') throw new Error('rootsForPaths 结果错: ' + needed);
  if (rootForPath(index, 'scripts/main.lua') !== ROOT_MAIN) throw new Error('rootForPath 主包路径错');
  // 容器路径必须能直接路由（引擎按 `pack/bundles/*.rlb` 读容器）。
  if (rootForPath(index, 'bundles/audio.rlb') !== 'pack-audio') {
    throw new Error('容器路径路由错: ' + rootForPath(index, 'bundles/audio.rlb'));
  }
  if (rootForPath(index, 'nope.png') !== null) throw new Error('计划外路径应返回 null');
  if (!isMainRoot(rootForPath(index, 'scripts/main.lua')) || isMainRoot('pack-audio')) {
    throw new Error('isMainRoot 判定错');
  }
  if (missingRoots(index, []).join(',') !== 'pack-audio,pack-sub') throw new Error('missingRoots 结果错');
  if (missingRoots(index, ['pack-sub']).join(',') !== 'pack-audio') throw new Error('已挂载的根不该再列');
  let threw = false;
  try {
    rootsForPaths(index, ['nope.png']);
  } catch (error) {
    threw = true;
  }
  if (!threw) throw new Error('未知路径必须报错');

  // 挂载编排：顺序、幂等、已挂载跳过、失败传播。
  const calls = [];
  const load = (root) => {
    calls.push(root);
    return Promise.resolve();
  };
  let first = null;
  return mountRoots(index, ['pack-sub', 'pack-audio'], { load }).then((result) => {
    first = result;
    if (calls.join(',') !== 'pack-sub,pack-audio') throw new Error('挂载顺序错: ' + calls);
    if (first.mountedNow.join(',') !== 'pack-sub,pack-audio') throw new Error('mountedNow 错');
    calls.length = 0;
    return mountRoots(index, ['pack-sub', 'pack-audio'], { load, mounted: first.mounted });
  }).then((second) => {
    if (calls.length !== 0 || second.mountedNow.length !== 0) throw new Error('已挂载的根不该重复挂');
    return ensurePathsMounted(index, ['assets/audio/bgm.mp3', 'scripts/main.lua'], { load });
  }).then((byPaths) => {
    if (byPaths.mountedNow.join(',') !== 'pack-audio') throw new Error('ensurePathsMounted 只挂路径所在的根: ' + byPaths.mountedNow);
    return mountRoots(index, ['pack-audio'], {
      load: () => Promise.reject(new Error('loadSubpackage failed')),
    }).then(() => {
      throw new Error('挂载失败必须向上抛（不许静默）');
    }, () => {
      console.log('bundle_roots_ok roots=' + Object.keys(index.roots).length + ' paths=' + Object.keys(index.pathRoot).length);
    });
  });
}

function main(argv) {
  const fs = require('fs');
  const args = argv.slice(2);
  if (args.includes('--self-test')) return selfTest();
  const planIndex = args.indexOf('--plan');
  if (planIndex === -1 || !args[planIndex + 1]) {
    console.error('用法: node plugins/wechat-minigame/bundle-roots.js --self-test | --plan <bundle_plan.json> [--paths a,b] [--json]');
    process.exit(2);
  }
  const index = planRootIndex(parsePlan(fs.readFileSync(args[planIndex + 1], 'utf8')));
  const pathsIndex = args.indexOf('--paths');
  const paths = pathsIndex !== -1 && args[pathsIndex + 1] ? args[pathsIndex + 1].split(',').filter(Boolean) : [];
  const report = {
    roots: Object.keys(index.roots).sort(),
    paths: Object.keys(index.pathRoot).length,
    needed: paths.length ? rootsForPaths(index, paths) : [],
    missing: missingRoots(index, []),
  };
  if (args.includes('--json')) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('bundle_roots_ok roots=' + report.roots.length + ' paths=' + report.paths + ' missing=' + report.missing.join(','));
  }
  return null;
}

if (typeof require !== 'undefined' && typeof module !== 'undefined' && require.main === module) {
  Promise.resolve().then(() => main(process.argv)).catch((error) => {
    console.error(error.message || String(error));
    process.exit(1);
  });
}

module.exports = {
  ROOT_MAIN,
  rootOfUnit,
  parsePlan,
  planRootIndex,
  rootsForPaths,
  rootForPath,
  isMainRoot,
  missingRoots,
  mountRoots,
  ensurePathsMounted,
};
