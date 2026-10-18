#!/usr/bin/env node
// 发布产物与计划的**一致性检查**（docs/67 §5.4）：bundle_plan 里声明的每个单元根，
// 在发布目录里都必须真的有目录、有入口 game.js、有它声明的容器。
//
// 为什么单独一个检查：`bundle-layout --verify` 只看 game.json 里写了的根；
// 这里反过来以**计划表**为准——计划说有 pack-audio，产物缺了就必须红，
// 否则适配层会在真机上 loadSubpackage 一个不存在的根。
//
// 用法：node scripts/build/check-bundle-mounts.js --launcher <发布根> [--plan <bundle_plan.json>] [--json]

'use strict';

const fs = require('fs');
const path = require('path');

const rootsModule = require(path.resolve(__dirname, '../../plugins/wechat-minigame/bundle-roots.js'));

function checkLauncher(launcherDir, planPath) {
  const planFile = planPath || path.join(launcherDir, 'pack', 'bundle_plan.json');
  if (!fs.existsSync(planFile)) throw new Error('缺计划表: ' + planFile);
  const plan = rootsModule.parsePlan(fs.readFileSync(planFile, 'utf8'));
  const index = rootsModule.planRootIndex(plan);
  const missing = [];
  for (const root of Object.keys(index.roots).sort()) {
    const rootDir = path.join(launcherDir, root);
    if (!fs.existsSync(rootDir)) {
      missing.push(root + '/');
      continue;
    }
    if (root !== rootsModule.ROOT_MAIN && !fs.existsSync(path.join(rootDir, 'game.js'))) {
      missing.push(root + '/game.js');
    }
    for (const bundle of index.roots[root].bundles) {
      if (!fs.existsSync(path.join(rootDir, bundle))) missing.push(root + '/' + bundle);
    }
  }
  return {
    roots: Object.keys(index.roots).sort(),
    paths: Object.keys(index.pathRoot).length,
    missing,
  };
}

function main(argv) {
  const args = argv.slice(2);
  const index = args.indexOf('--launcher');
  if (index === -1 || !args[index + 1]) {
    console.error('用法: node scripts/build/check-bundle-mounts.js --launcher <发布根> [--plan <plan>] [--json]');
    process.exit(2);
  }
  const planIndex = args.indexOf('--plan');
  const report = checkLauncher(args[index + 1], planIndex !== -1 ? args[planIndex + 1] : null);
  if (args.includes('--json')) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(
      `bundle_mounts_ok roots=${report.roots.length} paths=${report.paths} missing=${report.missing.length}`,
    );
  }
  if (report.missing.length > 0) {
    console.error('发布产物缺件（计划表声明了但目录里没有）:\n  ' + report.missing.join('\n  '));
    process.exit(1);
  }
}

if (require.main === module) main(process.argv);

module.exports = { checkLauncher };
