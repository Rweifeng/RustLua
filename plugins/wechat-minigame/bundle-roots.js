// 微信适配层的**多分包根路由表**（docs/67 §5.4）：由 `bundle_plan.json` 推出
// 「路径 → 包根」与「要挂哪些根」，与发布侧 `bundle-layout.js` 共用同一套根命名。
//
// 为什么单独一个模块：适配层（wechat-adapter.js）只该负责 IO/生命周期，
// 路由规则必须能被 Node 自测钉住（不需要微信开发者工具）。
//
// 约定（与 `distribution_layout` / `bundle-layout.js` 一致）：
//   unit=main     → mainpack（主包，随包存在，不需要 loadSubpackage）
//   unit=sub      → pack
//   unit=sub:<n>  → pack-<n>
//   unit=cdn      → null（发布器另行放置，不参与包内挂载）
//
// 用法：node bundle-roots.js --self-test | --plan <bundle_plan.json> [--paths a,b] [--json]

'use strict';

const fs = require('fs');

const ROOT_MAIN = 'mainpack';

/** 单元 → 包根；`null` 表示不参与包内挂载（cdn）。 */
function rootOfUnit(unit) {
  if (unit === 'main') return ROOT_MAIN;
  if (unit === 'sub') return 'pack';
  if (typeof unit === 'string' && /^sub:[A-Za-z0-9_-]{1,32}$/.test(unit)) {
    return 'pack-' + unit.slice(4);
  }
  if (unit === 'cdn') return null;
  throw new Error('不认识的 unit: ' + unit);
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
    const entry = (roots[root] ||= { unit: group.unit, bundles: [], paths: [] });
    if (typeof group.bundle === 'string' && group.bundle) entry.bundles.push(group.bundle);
    for (const path of group.paths) {
      if (Object.prototype.hasOwnProperty.call(pathRoot, path)) {
        throw new Error('路径同时属于多个组: ' + path);
      }
      pathRoot[path] = root;
    }
  }
  for (const key of Object.keys(roots)) roots[key].bundles.sort();
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
 */
async function mountRoots(index, roots, opts) {
  const load = opts && opts.load;
  if (typeof load !== 'function') throw new Error('mountRoots 需要 load(root)');
  const mounted = new Set((opts && opts.mounted) || []);
  const mountedNow = [];
  for (const root of roots) {
    if (root === ROOT_MAIN || mounted.has(root)) continue;
    if (!index.roots[root]) throw new Error('计划里没有这个根: ' + root);
    await load(root);
    mounted.add(root);
    mountedNow.push(root);
  }
  return { mounted: Array.from(mounted).sort(), mountedNow };
}

/** 读一组路径前，把这些路径所在的非主包根**按顺序**挂好。 */
async function ensurePathsMounted(index, paths, opts) {
  return mountRoots(index, rootsForPaths(index, paths), opts);
}

async function selfTest() {
  const plan = parsePlan(
    JSON.stringify({
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
    }),
  );
  const index = planRootIndex(plan);
  if (index.pathRoot['scripts/main.lua'] !== 'mainpack') throw new Error('main 路径应路由到 mainpack');
  if (index.pathRoot['assets/audio/bgm.mp3'] !== 'pack-audio') throw new Error('具名分包路径路由错');
  if (Object.prototype.hasOwnProperty.call(index.pathRoot, 'splash/splash.png')) {
    throw new Error('cdn 路径不该进入挂载路由');
  }
  const needed = rootsForPaths(index, ['assets/audio/bgm.mp3', 'scripts/main.lua']);
  if (needed.length !== 1 || needed[0] !== 'pack-audio') throw new Error('rootsForPaths 结果错: ' + needed);
  if (missingRoots(index, []).join(',') !== 'pack,pack-audio') throw new Error('missingRoots 结果错');
  if (missingRoots(index, ['pack']).join(',') !== 'pack-audio') throw new Error('已挂载的根不该再列');
  let threw = false;
  try {
    rootsForPaths(index, ['nope.png']);
  } catch (error) {
    threw = true;
  }
  if (!threw) throw new Error('未知路径必须报错');

  // 挂载编排：顺序、幂等、已挂载跳过、失败传播。
  const calls = [];
  const load = async (root) => {
    calls.push(root);
  };
  const first = await mountRoots(index, ['pack', 'pack-audio'], { load });
  if (calls.join(',') !== 'pack,pack-audio') throw new Error('挂载顺序错: ' + calls);
  if (first.mountedNow.join(',') !== 'pack,pack-audio') throw new Error('mountedNow 错');
  calls.length = 0;
  const second = await mountRoots(index, ['pack', 'pack-audio'], { load, mounted: first.mounted });
  if (calls.length !== 0 || second.mountedNow.length !== 0) throw new Error('已挂载的根不该重复挂');
  const byPaths = await ensurePathsMounted(index, ['assets/audio/bgm.mp3', 'scripts/main.lua'], { load });
  if (byPaths.mountedNow.join(',') !== 'pack-audio') throw new Error('ensurePathsMounted 只挂路径所在的根: ' + byPaths.mountedNow);
  let failed = false;
  try {
    await mountRoots(index, ['pack-audio'], {
      load: async () => {
        throw new Error('loadSubpackage failed');
      },
    });
  } catch (error) {
    failed = true;
  }
  if (!failed) throw new Error('挂载失败必须向上抛（不许静默）');
  console.log('bundle_roots_ok roots=' + Object.keys(index.roots).length + ' paths=' + Object.keys(index.pathRoot).length);
}

async function main(argv) {
  const args = argv.slice(2);
  if (args.includes('--self-test')) {
    await selfTest();
    return;
  }
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
}

if (require.main === module) {
  main(process.argv).catch((error) => {
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
  missingRoots,
  mountRoots,
  ensurePathsMounted,
};
