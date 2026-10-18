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
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes('--self-test')) {
    selfTest();
    return;
  }
  const fileIndex = args.indexOf('--file');
  if (fileIndex === -1 || !args[fileIndex + 1]) {
    console.error('用法: node scripts/build/bundle-layout.js --file <layout.json> [--json] | --self-test');
    process.exit(2);
  }
  const layout = parseLayout(fs.readFileSync(args[fileIndex + 1], 'utf8'));
  const plan = distributionPlan(layout);
  assertConservation(plan);
  if (args.includes('--json')) {
    console.log(JSON.stringify(plan, null, 2));
  } else {
    for (const move of plan.moves) console.log(`${move.from} -> ${move.to}`);
    console.log(`bundle_layout_ok moves=${plan.moves.length} subpackages=${plan.subpackages.length}`);
  }
}

if (require.main === module) main(process.argv);

module.exports = { parseLayout, distributionPlan, assertConservation, MAIN_ROOT, DEFAULT_SUB_ROOT };
