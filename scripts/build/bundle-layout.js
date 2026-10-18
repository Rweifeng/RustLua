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
//   unit=main      → mainpack/      （主包首屏）
//   unit=sub       → pack/          （微信默认分包根，name=pack）
//   unit=sub:<n>   → pack-<n>/      （具名分包根，name=pack-<n>）
//   cdn 不出现在布局里（发布器另行放置）

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const MAIN_ROOT = 'mainpack';
const DEFAULT_SUB_ROOT = 'pack';

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
 * 按搬运计划真正搬文件，并写 `game.json` 的 `subpackages` + 每个分包根的 `game.js` 入口。
 * 输入目录是**发布根**（含 `pack/`，即 `assetc` 产物落点）；`mainpack/`、`pack-<n>/` 由本函数创建。
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
    const src = path.join(launcherDir, 'pack', move.from);
    const dst = path.join(launcherDir, move.to);
    if (path.resolve(src) === path.resolve(dst)) continue;
    if (!fs.existsSync(src)) throw new Error('缺容器（应在 pack/ 下）: ' + src);
    fs.mkdirSync(path.dirname(dst), { recursive: true });
    fs.renameSync(src, dst);
    moved += 1;
  }
  game.subpackages = plan.subpackages;
  fs.writeFileSync(gamePath, JSON.stringify(game, null, 2) + '\n');
  // 微信规则：`subpackages[i].root` 目录下必须有入口 `game.js`；从默认分包入口复制一份。
  const template = path.join(launcherDir, 'pack', 'game.js');
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
    fs.mkdirSync(path.join(root, 'pack', 'bundles'), { recursive: true });
    fs.writeFileSync(path.join(root, 'pack', 'bundles', 'core.rlb'), 'core');
    fs.writeFileSync(path.join(root, 'pack', 'bundles', 'audio.rlb'), 'audio');
    fs.writeFileSync(path.join(root, 'pack', 'game.js'), '// entry\n');
    fs.writeFileSync(path.join(root, 'game.json'), JSON.stringify({ deviceOrientation: 'landscape' }));
    const layout = parseLayout(
      JSON.stringify({
        schema: 1,
        entries: 20,
        total_bytes: 10,
        roots: [
          { unit: 'main', root: 'mainpack', bundles: ['bundles/core.rlb'], entries: 10, bytes: 5 },
          { unit: 'sub:audio', root: 'pack-audio', bundles: ['bundles/audio.rlb'], entries: 10, bytes: 5 },
        ],
      }),
    );
    const plan = distributionPlan(layout);
    assertConservation(plan);
    const report = applyPlan(plan, root);
    if (report.moved !== 2) throw new Error('应搬 2 个容器，实际 ' + report.moved);
    if (!fs.existsSync(path.join(root, 'mainpack', 'bundles', 'core.rlb'))) throw new Error('主包容器没搬');
    if (!fs.existsSync(path.join(root, 'pack-audio', 'bundles', 'audio.rlb'))) throw new Error('分包容器没搬');
    if (!fs.existsSync(path.join(root, 'pack-audio', 'game.js'))) throw new Error('具名分包缺入口 game.js');
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
  const mainDir = path.join(launcherDir, 'mainpack');
  if (!fs.existsSync(mainDir)) throw new Error('缺 mainpack/（主包首屏目录）');
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
        { unit: 'main', root: 'mainpack', bundles: ['bundles/core.rlb'], entries: 10, bytes: 100 },
        { unit: 'sub', root: 'pack', bundles: ['bundles/art.rlb'], entries: 10, bytes: 100 },
        { unit: 'sub:audio', root: 'pack-audio', bundles: ['bundles/audio.rlb'], entries: 10, bytes: 100 },
      ],
    }),
  );
  const plan = distributionPlan(layout);
  if (plan.subpackages.length !== 2) throw new Error('subpackages 数量不对');
  if (plan.subpackages[0].name !== 'pack' || plan.subpackages[0].root !== 'pack') {
    throw new Error('默认分包应为 pack/pack');
  }
  if (plan.subpackages[1].name !== 'pack-audio' || plan.subpackages[1].root !== 'pack-audio') {
    throw new Error('具名分包应为 pack-audio/pack-audio');
  }
  if (plan.moves.find((move) => move.bundle === 'bundles/core.rlb').to !== 'mainpack/bundles/core.rlb') {
    throw new Error('主包容器应搬到 mainpack/');
  }
  if (plan.moves.find((move) => move.bundle === 'bundles/audio.rlb').to !== 'pack-audio/bundles/audio.rlb') {
    throw new Error('具名分包容器的落点不对');
  }
  assertConservation(plan);

  // 负例：total_bytes 对不上必须响亮失败。
  let threw = false;
  try {
    parseLayout(JSON.stringify({ schema: 1, total_bytes: 1, roots: [{ unit: 'main', root: 'mainpack', bundles: ['bundles/a.rlb'], bytes: 2 }] }));
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
