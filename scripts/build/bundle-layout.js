#!/usr/bin/env node
// 分发布局的**发布侧解析**（docs/67 §5.4/5.5）：把 `assetc layout --json` 的产物折算成
// 「容器文件搬到哪个目录」+「game.json 的 subpackages 列表」。
//
// 为什么单独一个模块：搬运逻辑要能被单测钉住（不需要微信开发者工具），
// 而 `first-screen-pack.js` 负责的是 IO/清单，不该再把目录规则写第二遍。
//
// 用法（CI/脚本）：
//   node scripts/build/bundle-layout.js --file <layout.json> [--json]
//   node scripts/build/bundle-layout.js --self-test
//
// 约定（与 Rust 侧 `distribution_layout` 一致）：
//   unit=main      → pack/          （主包首屏）
//   unit=sub       → pack/          （微信默认分包根，name=pack）
//   unit=sub:<n>   → pack-<n>/      （具名分包根，name=pack-<n>）
//   cdn 不出现在布局里（发布器另行放置）

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const MAIN_ROOT = 'pack';
const DEFAULT_SUB_ROOT = 'pack-sub';
// 微信代码包硬门：主包 ≤ 4 MB、主包 + 全部分包 ≤ 20 MB。`assetc layout` 已经校验过；
// 这里再校一次是防"手改/旧工具生成的 layout.json"绕过发布期唯一的配额门。
const MAX_MAIN_BYTES = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES = 20 * 1024 * 1024;

/** 解析 `assetc layout --json` 的输出；结构不对就响亮失败。 */
function parseLayout(raw) {
  // PowerShell 的 `Out-File -Encoding utf8` 会带 BOM；这里统一吃掉（发布脚本常用重定向）。
  const text = String(raw).replace(/^\uFEFF/, '');
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    throw new Error('分发布局不是合法 JSON: ' + error.message);
  }
  if (!value || value.schema !== 1) throw new Error('分发布局 schema 不是 1');
  if (!Array.isArray(value.roots) || value.roots.length === 0) {
    throw new Error('分发布局没有 roots');
  }
  const seen = new Set();
  let bytes = 0;
  let entries = 0;
  for (const root of value.roots) {
    if (!root || typeof root.root !== 'string' || typeof root.unit !== 'string') {
      throw new Error('roots 里缺少 unit/root');
    }
    if (!['main', 'sub'].includes(root.unit) && !/^sub:[A-Za-z0-9_-]{1,32}$/.test(root.unit)) {
      throw new Error('不认识的 unit: ' + root.unit);
    }
    if (seen.has(root.unit)) throw new Error('unit 重复: ' + root.unit);
    seen.add(root.unit);
    bytes += root.bytes || 0;
    entries += root.entries || 0;
    if (!Array.isArray(root.bundles) || root.bundles.length === 0) {
      throw new Error('root ' + root.root + ' 没有容器');
    }
    for (const bundle of root.bundles) {
      if (typeof bundle !== 'string' || !bundle.startsWith('bundles/')) {
        throw new Error('容器路径不在 bundles/ 下: ' + bundle);
      }
    }
  }
  if (value.total_bytes !== undefined && value.total_bytes !== bytes) {
    throw new Error(`total_bytes 与 roots 之和不符: ${value.total_bytes} != ${bytes}`);
  }
  if (value.entries !== undefined && value.entries !== entries) {
    throw new Error(`entries 与 roots 之和不符: ${value.entries} != ${entries}`);
  }
  const mainRoot = value.roots.find((root) => root.unit === 'main');
  if (mainRoot && (mainRoot.bytes || 0) > MAX_MAIN_BYTES) {
    throw new Error(`主包（pack）落盘 ${Math.round((mainRoot.bytes || 0) / 1024)} KB 超过 4,096 KB：把非启动资源挪到分包`);
  }
  if (bytes > MAX_TOTAL_BYTES) {
    throw new Error(`主包 + 全部分包落盘 ${Math.round(bytes / 1024)} KB 超过总量上限 20,480 KB`);
  }
  return { schema: value.schema, roots: value.roots, totalBytes: bytes, entries };
}

/**
 * 把布局折算成搬运计划与 `game.json` 的分包声明。
 * `bundle` 是包内相对路径（`bundles/x.rlb`），`to` 是发布根下的相对路径。
 */
function distributionPlan(layout) {
  const moves = [];
  const subpackages = [];
  const roots = [];
  for (const root of layout.roots) {
    if (root.unit === 'main') {
      for (const bundle of root.bundles) moves.push({ bundle, from: bundle, to: `${MAIN_ROOT}/${bundle}` });
      roots.push({ unit: 'main', root: MAIN_ROOT, bytes: root.bytes });
      continue;
    }
    // 具名分包：root 名由 unit 后缀决定；默认分包固定 pack。
    const name = root.unit === 'sub' ? DEFAULT_SUB_ROOT : `pack-${root.unit.slice(4)}`;
    if (root.root !== name) {
      throw new Error(`布局的 root(${root.root}) 与 unit(${root.unit}) 不一致`);
    }
    for (const bundle of root.bundles) moves.push({ bundle, from: bundle, to: `${name}/${bundle}` });
    subpackages.push({ name, root: name });
    roots.push({ unit: root.unit, root: name, bytes: root.bytes });
  }
  return { moves, subpackages, roots, totalBytes: layout.totalBytes, entries: layout.entries };
}

/** 字节守恒：搬走的容器合计必须等于布局合计（不是"看起来差不多"）。 */
function assertConservation(plan) {
  const moved = plan.moves.length;
  if (moved === 0) throw new Error('搬运计划为空');
  const total = plan.roots.reduce((sum, root) => sum + (root.bytes || 0), 0);
  if (total !== plan.totalBytes) {
    throw new Error(`搬运计划字节不守恒: ${total} != ${plan.totalBytes}`);
  }
  return true;
}

/**
 * 找容器文件：发布链会把 `.rlb` 按微信代码包白名单改名为 `.rlb.txt`
 * （`code-package.json` 记录同一份映射）。apply 发生在改名之后，所以两种都要认；
 * 认到 `.txt` 时**目标名也带 `.txt`**，运行时仍按逻辑名 `.rlb` 查表得到宿主名。
 */
function resolveContainer(launcherDir, bundle) {
  const base = path.join(launcherDir, 'pack-sub', bundle);
  if (fs.existsSync(base)) return base;
  if (fs.existsSync(base + '.txt')) return base + '.txt';
  // `-MultiSub` 在首屏切分之后运行：主包容器已在 pack/，这里同时接受它作为源。
  const mainBase = path.join(launcherDir, 'pack', bundle);
  if (fs.existsSync(mainBase)) return mainBase;
  if (fs.existsSync(mainBase + '.txt')) return mainBase + '.txt';
  return null;
}

/**
 * 按搬运计划真正搬文件，并写 `game.json` 的 `subpackages` + 每个分包根的 `game.js` 入口。
 * 输入目录是**发布根**（含 `pack-sub/`，即 `assetc` 产物落点）；`pack/`、`pack-<n>/` 由本函数创建。
 * 返回计数用于日志断言（`moved` 只数真正跨目录移动的容器）。
 */
function applyPlan(plan, launcherDir) {
  const gamePath = path.join(launcherDir, 'game.json');
  let game = {};
  if (fs.existsSync(gamePath)) {
    game = JSON.parse(fs.readFileSync(gamePath, 'utf8').replace(/^\uFEFF/, ''));
  }
  let moved = 0;
  for (const move of plan.moves) {
    const src = resolveContainer(launcherDir, move.from);
    if (!src) throw new Error('缺容器（应在 pack-sub/ 下）: ' + path.join(launcherDir, 'pack-sub', move.from));
    const renamed = src.endsWith('.txt');
    const dst = path.join(launcherDir, move.to + (renamed ? '.txt' : ''));
    if (path.resolve(src) === path.resolve(dst)) continue;
    fs.mkdirSync(path.dirname(dst), { recursive: true });
    fs.renameSync(src, dst);
    moved += 1;
  }
  game.subpackages = plan.subpackages;
  // pack-wasm 是发布链的独立内核根（不在 assetc 的资源组里）：它已经存在时，game.json
  // 必须继续声明它，否则 `bundle-layout --apply` 会把默认的 wasm 根从分包列表里删掉。
  const wasmRoot = fs.existsSync(path.join(launcherDir, 'pack-wasm', 'game_web_bg.wasm'))
    ? 'pack-wasm'
    : '';
  if (wasmRoot && !game.subpackages.some((sub) => sub && sub.root === wasmRoot)) {
    game.subpackages.push({ name: wasmRoot, root: wasmRoot });
  }
  fs.writeFileSync(gamePath, JSON.stringify(game, null, 2) + '\n');
  // 计划表必须留在**主包可读**的位置：适配层靠它决定"某个路径去哪个根、要挂哪些根"，
  // 而分包根未挂载前读不到自己目录里的文件。pack/ 里那份继续留给工具链对账。
  const planSrc = path.join(launcherDir, 'pack-sub', 'bundle_plan.json');
  if (fs.existsSync(planSrc)) {
    // 根计划表加一个**只有 launcher 根才有的标记**：同一份 bundle_plan.json 也留在 pack/ 里
    // 给工具链对账；`sub-root` 宿主会把根路径回落到 pack/，适配层若只看文件名就会把单分包
    // 产物误判成多根（实测 verify 的 sub-root 档）。标记让"根计划"与"分包里的计划"可区分。
    const planObject = JSON.parse(fs.readFileSync(planSrc, 'utf8'));
    planObject.launcher_root = true;
    if (wasmRoot) planObject.wasm_root = wasmRoot;
    fs.writeFileSync(path.join(launcherDir, 'bundle_plan.json'),
      JSON.stringify(planObject, null, 2) + '\n');
  }
  // 微信规则：`subpackages[i].root` 目录下必须有入口 `game.js`；从默认分包入口复制一份。
  const template = path.join(launcherDir, 'pack-sub', 'game.js');
  for (const sub of plan.subpackages) {
    const target = path.join(launcherDir, sub.root, 'game.js');
    if (fs.existsSync(target)) continue;
    if (!fs.existsSync(template)) throw new Error('缺分包入口模板: ' + template);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(template, target);
  }
  return { moved, roots: plan.roots.length, subpackages: plan.subpackages.length };
}

/** `applyPlan` 的自测：临时目录里造 pack/，搬完检查文件位置、game.json 与入口。 */
function applySelfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bundle-layout-'));
  try {
    fs.mkdirSync(path.join(root, 'pack-sub', 'bundles'), { recursive: true });
    fs.writeFileSync(path.join(root, 'pack-sub', 'bundles', 'core.rlb'), 'core');
    // 具名分包用**改名后**的 `.rlb.txt`：apply 发生在代码包后缀改名之后，必须照样认得。
    fs.writeFileSync(path.join(root, 'pack-sub', 'bundles', 'audio.rlb.txt'), 'audio');
    fs.writeFileSync(path.join(root, 'pack-sub', 'game.js'), '// entry\n');
    // apply 必须把计划表复制到 launcher 根（适配层在挂载任何分包之前就要读它）。
    fs.writeFileSync(path.join(root, 'pack-sub', 'bundle_plan.json'), '{"schema":1}');
    fs.writeFileSync(path.join(root, 'game.json'), JSON.stringify({ deviceOrientation: 'landscape' }));
    const layout = parseLayout(
      JSON.stringify({
        schema: 1,
        entries: 20,
        total_bytes: 10,
        roots: [
          { unit: 'main', root: 'pack', bundles: ['bundles/core.rlb'], entries: 10, bytes: 5 },
          { unit: 'sub:audio', root: 'pack-audio', bundles: ['bundles/audio.rlb'], entries: 10, bytes: 5 },
        ],
      }),
    );
    const plan = distributionPlan(layout);
    assertConservation(plan);
    const report = applyPlan(plan, root);
    if (report.moved !== 2) throw new Error('应搬 2 个容器，实际 ' + report.moved);
    if (!fs.existsSync(path.join(root, 'pack', 'bundles', 'core.rlb'))) throw new Error('主包容器没搬');
    if (!fs.existsSync(path.join(root, 'pack-audio', 'bundles', 'audio.rlb.txt'))) {
      throw new Error('分包容器没搬（改名后的 .rlb.txt 必须保留后缀）');
    }
    if (!fs.existsSync(path.join(root, 'pack-audio', 'game.js'))) throw new Error('具名分包缺入口 game.js');
    if (!fs.existsSync(path.join(root, 'bundle_plan.json'))) throw new Error('计划表没复制到 launcher 根');
    const rootPlan = JSON.parse(fs.readFileSync(path.join(root, 'bundle_plan.json'), 'utf8'));
    if (rootPlan.launcher_root !== true) throw new Error('launcher 根计划表缺 launcher_root 标记');
    const game = JSON.parse(fs.readFileSync(path.join(root, 'game.json'), 'utf8'));
    if (game.subpackages.length !== 1 || game.subpackages[0].root !== 'pack-audio') {
      throw new Error('game.json 的 subpackages 不对: ' + JSON.stringify(game.subpackages));
    }
    if (game.deviceOrientation !== 'landscape') throw new Error('game.json 其它字段被丢了');
    const shape = verifyLauncher(root);
    if (shape.subBundles !== 1 || shape.mainBundles !== 1) {
      throw new Error('发布根形状计数不对: ' + JSON.stringify(shape));
    }
    // 负例：删掉具名分包入口，必须响亮失败。
    fs.rmSync(path.join(root, 'pack-audio', 'game.js'));
    let threw = false;
    try {
      verifyLauncher(root);
    } catch (error) {
      threw = true;
    }
    if (!threw) throw new Error('缺分包入口必须被拒绝');
    console.log('bundle_layout_apply_ok moved=' + report.moved + ' subpackages=' + report.subpackages);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

/** 校验发布根形状：每个分包根有 `game.js` 入口、能数出容器；主包目录存在。 */
function verifyLauncher(launcherDir) {
  const gamePath = path.join(launcherDir, 'game.json');
  if (!fs.existsSync(gamePath)) throw new Error('缺 game.json: ' + gamePath);
  const game = JSON.parse(fs.readFileSync(gamePath, 'utf8').replace(/^\uFEFF/, ''));
  const subs = game.subpackages || [];
  if (!Array.isArray(subs)) throw new Error('game.json 的 subpackages 不是数组');
  const countBundles = (dir) =>
    fs.existsSync(dir)
      ? fs.readdirSync(dir).filter((name) => name.endsWith('.rlb') || name.endsWith('.rlb.txt')).length
      : 0;
  let subBundles = 0;
  for (const sub of subs) {
    if (!sub || typeof sub.name !== 'string' || typeof sub.root !== 'string') {
      throw new Error('subpackages 条目缺 name/root');
    }
    if (!fs.existsSync(path.join(launcherDir, sub.root, 'game.js'))) {
      throw new Error(`分包 ${sub.name} 缺入口 game.js（root=${sub.root}）`);
    }
    subBundles += countBundles(path.join(launcherDir, sub.root, 'bundles'));
  }
  const mainDir = path.join(launcherDir, 'pack');
  if (!fs.existsSync(mainDir)) throw new Error('缺 pack/（主包首屏目录）');
  const mainBundles = countBundles(path.join(mainDir, 'bundles'));
  return { subpackages: subs.length, subBundles, mainBundles };
}

function selfTest() {
  const layout = parseLayout(
    JSON.stringify({
      schema: 1,
      entries: 30,
      total_bytes: 300,
      roots: [
        { unit: 'main', root: 'pack', bundles: ['bundles/core.rlb'], entries: 10, bytes: 100 },
        { unit: 'sub', root: 'pack-sub', bundles: ['bundles/art.rlb'], entries: 10, bytes: 100 },
        { unit: 'sub:audio', root: 'pack-audio', bundles: ['bundles/audio.rlb'], entries: 10, bytes: 100 },
      ],
    }),
  );
  const plan = distributionPlan(layout);
  if (plan.subpackages.length !== 2) throw new Error('subpackages 数量不对');
  if (plan.subpackages[0].name !== 'pack-sub' || plan.subpackages[0].root !== 'pack-sub') {
    throw new Error('默认分包应为 pack-sub/pack-sub');
  }
  if (plan.subpackages[1].name !== 'pack-audio' || plan.subpackages[1].root !== 'pack-audio') {
    throw new Error('具名分包应为 pack-audio/pack-audio');
  }
  if (plan.moves.find((move) => move.bundle === 'bundles/core.rlb').to !== 'pack/bundles/core.rlb') {
    throw new Error('主包容器应搬到 pack/');
  }
  if (plan.moves.find((move) => move.bundle === 'bundles/audio.rlb').to !== 'pack-audio/bundles/audio.rlb') {
    throw new Error('具名分包容器的落点不对');
  }
  assertConservation(plan);

  // 负例：total_bytes 对不上必须响亮失败。
  let threw = false;
  try {
    parseLayout(JSON.stringify({ schema: 1, total_bytes: 1, roots: [{ unit: 'main', root: 'pack', bundles: ['bundles/a.rlb'], bytes: 2 }] }));
  } catch (error) {
    threw = true;
  }
  if (!threw) throw new Error('字节不守恒必须被拒绝');

  console.log('bundle_layout_ok moves=' + plan.moves.length + ' subpackages=' + plan.subpackages.length);
  applySelfTest();
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes('--self-test')) {
    selfTest();
    return;
  }
  const verifyIndex = args.indexOf('--verify');
  if (verifyIndex !== -1 && args[verifyIndex + 1]) {
    const shape = verifyLauncher(args[verifyIndex + 1]);
    console.log(
      `bundle_layout_verify_ok subpackages=${shape.subpackages} main_bundles=${shape.mainBundles} sub_bundles=${shape.subBundles}`,
    );
    return;
  }
  const fileIndex = args.indexOf('--file');
  if (fileIndex === -1 || !args[fileIndex + 1]) {
    console.error(
      '用法: node scripts/build/bundle-layout.js --file <layout.json> [--json] [--apply <发布根>] | --self-test',
    );
    process.exit(2);
  }
  const layout = parseLayout(fs.readFileSync(args[fileIndex + 1], 'utf8'));
  const plan = distributionPlan(layout);
  assertConservation(plan);
  const applyIndex = args.indexOf('--apply');
  if (applyIndex !== -1 && args[applyIndex + 1]) {
    const report = applyPlan(plan, args[applyIndex + 1]);
    console.log(
      `bundle_layout_applied moved=${report.moved} roots=${report.roots} subpackages=${report.subpackages}`,
    );
    return;
  }
  if (args.includes('--json')) {
    console.log(JSON.stringify(plan, null, 2));
  } else {
    for (const move of plan.moves) console.log(`${move.from} -> ${move.to}`);
    console.log(`bundle_layout_ok moves=${plan.moves.length} subpackages=${plan.subpackages.length}`);
  }
}

if (require.main === module) main(process.argv);

module.exports = {
  parseLayout,
  distributionPlan,
  assertConservation,
  applyPlan,
  verifyLauncher,
  MAIN_ROOT,
  DEFAULT_SUB_ROOT,
};
