#!/usr/bin/env node
// 发布产物与计划表的**一致性检查**（docs/67 §5.4）：计划表里声明的每个单元根，在发布目录里
// 都必须真的有目录、有入口 game.js、有它声明的容器。
//
// 为什么单独一个检查：`bundle-layout --verify` 只看 game.json 里写了的根；这里反过来以
// **计划表**为准——计划说有 pack-audio，产物缺了就必须红，否则适配层会在真机上
// loadSubpackage 一个不存在的根。
//
// 这个检查**不解析容器、也不自己推改名**：
//   - 计划表只有**一个**权威位置：发布根的 `bundle_plan.json`——适配层启动时读的就是它；
//   - 改名只有**一份**规则：`scripts/lib/code-package-rules.js`。产物自带
//     `code-package.json` 的 `renamed` 表时以**它**为权威（那是发布链真实写下的结果，
//     规则模块只是它的生成器）。
//
// 早先这里写着 `bundle + '.txt'`、并且在发布根缺计划表时回落到 `pack/bundle_plan.json`：
// 前者是第二套改名假设（白名单规则一改就静默错位），后者是第二个"计划表在哪"的猜测
// （真实产物里 `pack/` 与 `pack-sub/` 各有一份副本，挑哪份全凭顺序，陈的那份会让门变绿）。
//
// 用法：node scripts/build/check-bundle-mounts.js --launcher <发布根> [--plan <bundle_plan.json>] [--json]
//       node scripts/build/check-bundle-mounts.js --self-test

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const rules = require(path.resolve(__dirname, '../lib/code-package-rules.js'));
const rootsModule = require(path.resolve(__dirname, '../../plugins/wechat-minigame/bundle-roots.js'));

const PLAN_FILE = 'bundle_plan.json';
const RENAME_MAP_FILE = 'code-package.json';

/** 读计划表：只认传入路径或发布根那一份，**不猜**第二个位置。 */
function loadPlan(launcherDir, planPath) {
  const file = planPath ? path.resolve(planPath) : path.join(launcherDir, PLAN_FILE);
  if (!fs.existsSync(file)) {
    throw new Error(
      '缺计划表: ' + file +
        '（发布根的计划表是适配层启动时读的那一份；assetc 的原始落点 pack/bundle_plan.json ' +
        '不算——副本可能比它新，也可能更陈）',
    );
  }
  return { file, plan: rootsModule.parsePlan(fs.readFileSync(file, 'utf8')) };
}

/** 读产物自带的改名表（发布链写的）；没有就返回 `null`，由规则模块兜。 */
function loadRenameMap(launcherDir) {
  const file = path.join(launcherDir, RENAME_MAP_FILE);
  if (!fs.existsSync(file)) return { file: null, renamed: null };
  const text = String(fs.readFileSync(file, 'utf8')).replace(/^\uFEFF/, '');
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw new Error(RENAME_MAP_FILE + ' 不是合法 JSON: ' + error.message);
  }
  const renamed = parsed && parsed.renamed && typeof parsed.renamed === 'object' ? parsed.renamed : {};
  return { file, renamed };
}

/**
 * 容器在产物里的**宿主文件名**：
 *   - 产物改名表里有这条 → 只认它（发布链写下的就是真机上的名字）；
 *   - 否则 → 规则模块算（`.rlb` 不在白名单 ⇒ `xxx.rlb.txt`），并把原名也当候选
 *     （未走发布链的夹具里容器还是原名）。
 */
function hostNamesOf(bundle, renamed) {
  if (renamed && Object.prototype.hasOwnProperty.call(renamed, bundle)) {
    const hosted = renamed[bundle];
    if (typeof hosted !== 'string' || !hosted) {
      throw new Error(RENAME_MAP_FILE + ' 里 ' + bundle + ' 的改名结果不是字符串: ' + hosted);
    }
    return [hosted];
  }
  const hosted = rules.hostNameFor(bundle);
  return hosted === bundle ? [bundle] : [bundle, hosted];
}

// `remoteDir`：瘦首包（`-AssetPack remote`）的资源包根。这时容器**不在**发布根里，
// 而在随发布一起产出的 `<发布根>/assets/pack-sub/bundles/…`（整目录传 CDN）。
// 不给它时，缺件就是缺件；给了它时，本地缺的容器必须能在资源包里找到——两边都没有才算缺件
// （"远端没找到就算了"与"本地有就行"都不成立，那正是真机 404 的两个来源）。
function checkLauncher(launcherDir, planPath, remoteDir) {
  const loaded = loadPlan(launcherDir, planPath);
  const rename = loadRenameMap(launcherDir);
  const index = rootsModule.planRootIndex(loaded.plan);
  const missing = [];
  let containers = 0;
  let renamedContainers = 0;
  let remoteContainers = 0;
  for (const root of Object.keys(index.roots).sort()) {
    const rootDir = path.join(launcherDir, root);
    const rootExists = fs.existsSync(rootDir);
    // 瘦首包（`-AssetPack remote`）里发布根本来就没有容器根：骨架只有 `pack-sub/`，
    // 容器全在资源包里。给了 `--remote` 时按"远端形态"核；没给就照旧判缺根。
    if (!rootExists && !remoteDir) {
      missing.push(root + '/');
      continue;
    }
    if (rootExists && root !== rootsModule.ROOT_MAIN && !fs.existsSync(path.join(rootDir, 'game.js'))) {
      missing.push(root + '/game.js');
    }
    for (const bundle of index.roots[root].bundles) {
      containers += 1;
      const candidates = hostNamesOf(bundle, rename.renamed);
      const found = rootExists
        ? candidates.find((name) => fs.existsSync(path.join(rootDir, name)))
        : undefined;
      if (found) {
        if (found !== bundle) renamedContainers += 1;
        continue;
      }
      // 远端（CDN）那份**保持逻辑名**：改名是微信代码包白名单的产物，CDN 不受约束。
      // 所以远端候选 = 逻辑名(+.txt) ∪ 改名表候选，而不是照抄本地候选。
      const remoteCandidates = remoteDir
        ? Array.from(new Set([bundle, bundle + '.txt', ...candidates]))
        : [];
      const inRemote = remoteCandidates.find((name) => fs.existsSync(path.join(remoteDir, name)));
      if (inRemote) {
        remoteContainers += 1;
        continue;
      }
      missing.push(root + '/' + candidates.join(' 或 ' + root + '/'));
    }
  }
  return {
    plan: path.relative(launcherDir, loaded.file) || PLAN_FILE,
    rename_map: rename.file ? RENAME_MAP_FILE : null,
    roots: Object.keys(index.roots).sort(),
    paths: Object.keys(index.pathRoot).length,
    containers,
    renamed_containers: renamedContainers,
    remote_containers: remoteContainers,
    missing,
  };
}

// ---------------------------------------------------------------- self-test

function writeFile(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

function selfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bundle-mounts-'));
  const planOf = (groups) =>
    JSON.stringify({
      schema: 1,
      format: 'assetc-bundle-plan-1',
      block_size: 262144,
      groups,
      checksum: '',
    });
  const groups = [
    {
      name: 'core',
      unit: 'main',
      bundle: 'bundles/core.rlb',
      entries: 1,
      raw_bytes: 1,
      bundle_bytes: 1,
      hash: '',
      paths: ['scripts/main.lua'],
    },
    {
      name: 'audio',
      unit: 'sub:audio',
      bundle: 'bundles/audio.rlb',
      entries: 1,
      raw_bytes: 1,
      bundle_bytes: 1,
      hash: '',
      paths: ['assets/audio/bgm.mp3'],
    },
  ];
  const fresh = () => {
    fs.rmSync(root, { recursive: true, force: true });
    fs.mkdirSync(root, { recursive: true });
    writeFile(path.join(root, 'bundle_plan.json'), planOf(groups));
    writeFile(path.join(root, 'pack', 'bundles', 'core.rlb.txt'), 'c');
    writeFile(path.join(root, 'pack-audio', 'game.js'), '');
    writeFile(path.join(root, 'pack-audio', 'bundles', 'audio.rlb.txt'), 'a');
  };
  const expectMissing = (label, expected) => {
    let report;
    try {
      report = checkLauncher(root);
    } catch (error) {
      throw new Error(label + '：不该抛错，得到 ' + error.message);
    }
    if (report.missing.length === 0) throw new Error(label + '：必须报缺件');
    if (expected && !report.missing.join(' | ').includes(expected)) {
      throw new Error(label + '：缺件信息里应提到 ' + expected + '，得到 ' + report.missing.join(' | '));
    }
  };

  // 1) 正例：多根齐全、容器按白名单改名（无 code-package.json，走规则模块）。
  fresh();
  const ok = checkLauncher(root);
  if (ok.missing.length !== 0) throw new Error('正例不该缺件: ' + ok.missing.join(' | '));
  if (ok.roots.join(',') !== 'pack,pack-audio') throw new Error('根集合错: ' + ok.roots);
  if (ok.containers !== 2 || ok.renamed_containers !== 2) {
    throw new Error('容器计数错: ' + JSON.stringify([ok.containers, ok.renamed_containers]));
  }

  // 2) 反例：缺一个具名分包的容器 —— 必须点名根与容器名。
  fresh();
  fs.rmSync(path.join(root, 'pack-audio', 'bundles', 'audio.rlb.txt'));
  expectMissing('缺具名分包容器', 'pack-audio/bundles/audio.rlb');

  // 3) 反例：根目录缺 game.js（`loadSubpackage` 会失败）。
  fresh();
  fs.rmSync(path.join(root, 'pack-audio', 'game.js'));
  expectMissing('缺分包入口', 'pack-audio/game.js');

  // 4) 反例：发布链的改名表写的是 `.txt`，产物里却只有原名 —— 真机读不到，必须红。
  //    这条钉住"以产物自带改名表为权威"，不再自己猜后缀。
  fresh();
  fs.rmSync(path.join(root, 'pack-audio', 'bundles', 'audio.rlb.txt'));
  writeFile(path.join(root, 'pack-audio', 'bundles', 'audio.rlb'), 'a');
  writeFile(
    path.join(root, RENAME_MAP_FILE),
    JSON.stringify({ version: 1, renamed: { 'bundles/audio.rlb': 'bundles/audio.rlb.txt' } }),
  );
  expectMissing('改名表与产物不符', 'bundles/audio.rlb.txt');

  // 5) 反例：发布根没有计划表，只在 pack/ 里留了一份副本 —— 不许拿副本冒充。
  fresh();
  fs.renameSync(path.join(root, 'bundle_plan.json'), path.join(root, 'pack', PLAN_FILE));
  let message = '';
  try {
    checkLauncher(root);
  } catch (error) {
    message = String(error.message || error);
  }
  if (!message.includes('缺计划表')) throw new Error('缺计划表必须响亮失败，得到: ' + (message || '<没有抛错>'));
  if (!message.includes('pack/bundle_plan.json')) {
    throw new Error('缺计划表的报错应说明为什么不用 pack/ 副本，得到: ' + message);
  }

  // 6) 瘦首包（`-AssetPack remote`）：容器全在资源包（`<发布根>/assets/pack-sub/`），发布根里一个都没有。
  //    不给 `--remote` 必须报缺件（不许因为"是 remote 形态"就整类放过）；给了就必须全在远端找到。
  fresh();
  const remoteDir = path.join(root, 'assets', 'pack-sub');
  fs.mkdirSync(path.join(remoteDir, 'bundles'), { recursive: true });
  fs.renameSync(path.join(root, 'pack', 'bundles', 'core.rlb.txt'), path.join(remoteDir, 'bundles', 'core.rlb.txt'));
  fs.renameSync(path.join(root, 'pack-audio', 'bundles', 'audio.rlb.txt'), path.join(remoteDir, 'bundles', 'audio.rlb.txt'));
  const thinLocal = checkLauncher(root);
  if (thinLocal.missing.filter((item) => item.includes('bundles/')).length !== 2) {
    throw new Error('瘦首包不给 --remote 必须报两个容器缺件，得到: ' + thinLocal.missing.join(' | '));
  }
  const thinRemote = checkLauncher(root, null, remoteDir);
  if (thinRemote.missing.length !== 0) throw new Error('瘦首包给了 --remote 不该缺件: ' + thinRemote.missing.join(' | '));
  if (thinRemote.remote_containers !== 2) {
    throw new Error('瘦首包的 remote 容器计数错: ' + thinRemote.remote_containers);
  }

  fs.rmSync(root, { recursive: true, force: true });
  console.log('bundle_mounts_selftest_ok cases=6');
  return { missing: [] };
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes('--self-test')) {
    selfTest();
    return;
  }
  const index = args.indexOf('--launcher');
  if (index === -1 || !args[index + 1]) {
    console.error(
      '用法: node scripts/build/check-bundle-mounts.js --launcher <发布根> [--plan <plan>] [--remote <资源包根>] [--json] | --self-test',
    );
    process.exit(2);
  }
  const planIndex = args.indexOf('--plan');
  const remoteIndex = args.indexOf('--remote');
  const report = checkLauncher(
    args[index + 1],
    planIndex !== -1 ? args[planIndex + 1] : null,
    remoteIndex !== -1 ? args[remoteIndex + 1] : null,
  );
  if (args.includes('--json')) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(
      `bundle_mounts_ok roots=${report.roots.length} paths=${report.paths} containers=${report.containers}` +
        ` renamed=${report.renamed_containers} remote=${report.remote_containers} missing=${report.missing.length}`,
    );
  }
  if (report.missing.length > 0) {
    console.error('发布产物缺件（计划表声明了但目录里没有）:\n  ' + report.missing.join('\n  '));
    process.exit(1);
  }
}

if (require.main === module) main(process.argv);

module.exports = { checkLauncher, loadPlan, loadRenameMap, hostNamesOf };
