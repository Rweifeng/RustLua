#!/usr/bin/env node
// 发布期 boot 闭包检查（docs/67 §5.8）：计划表里的 `boot` 元数据必须与真实产物一致，
// 而不是靠"main 单元看起来够用"的约定。
//
// 用法：
//   node scripts/build/check-boot-closure.js --launcher <发布根> [--trace <load-trace.json>]
//
// 检查项：
//   1. 根计划表存在 `boot.roots/groups/entries/wasm`，且 `wasm_root` 在 boot roots 里；
//   2. boot.groups 恰好是计划里 `unit = "main"` 的组，不能把 sub/cdn 组混进来；
//   3. pack.json 的 entries 与 boot.entries 完全相同（少一条就是首屏漏列）；
//   4. 每条 boot entry / 每个 main 容器在 mainpack/（或 boot root）里真实存在；
//   5. game.json 声明了 boot 的非 main root（pack-wasm）；
//   6. 给了 --trace 时：boot（含启动场景）读到的每条路径都必须归到 main 单元。

'use strict';

const fs = require('fs');
const path = require('path');

const MAIN_ROOT = 'pack';
const rootsModule = require(path.resolve(__dirname, '../../plugins/wechat-minigame/bundle-roots.js'));

function parseArgs(argv) {
  const args = { launcher: '', trace: '' };
  for (let i = 2; i < argv.length; i += 1) {
    if (argv[i] === '--launcher') args.launcher = argv[++i] || '';
    else if (argv[i] === '--trace') args.trace = argv[++i] || '';
    else throw new Error('未知参数: ' + argv[i]);
  }
  if (!args.launcher) throw new Error('用法: node scripts/build/check-boot-closure.js --launcher <发布根> [--trace <load-trace.json>]');
  return args;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8').replace(/^\uFEFF/, ''));
}

function existsAny(root, rel) {
  return fs.existsSync(path.join(root, rel)) || fs.existsSync(path.join(root, rel + '.txt'));
}

function sorted(values) {
  return values.slice().sort();
}

function checkLauncher(launcherDir, tracePath) {
  const failures = [];
  const root = path.resolve(launcherDir);
  const planFile = path.join(root, 'bundle_plan.json');
  if (!fs.existsSync(planFile)) throw new Error('缺 launcher 根 bundle_plan.json: ' + planFile);
  const plan = rootsModule.parsePlan(fs.readFileSync(planFile, 'utf8'));
  const boot = plan.boot;
  if (!boot || typeof boot !== 'object') {
    throw new Error('计划表缺少 boot 元数据（发布器没有写 boot 闭包）');
  }
  if (!Array.isArray(boot.roots) || boot.roots.length === 0) failures.push('boot.roots 为空');
  if (!Array.isArray(boot.groups)) failures.push('boot.groups 不是数组');
  if (!Array.isArray(boot.entries)) failures.push('boot.entries 不是数组');
  const wasmRoot = String(plan.wasm_root || '');
  if (!wasmRoot) failures.push('计划表缺少 wasm_root');
  else if (!(boot.roots || []).includes(wasmRoot)) failures.push(`boot.roots 不包含 wasm_root=${wasmRoot}`);

  const mainGroups = plan.groups.filter((group) => group.unit === 'main').map((group) => group.name);
  if (JSON.stringify(sorted(boot.groups || [])) !== JSON.stringify(sorted(mainGroups))) {
    failures.push(`boot.groups 与 main 单元不一致: boot=${JSON.stringify(boot.groups || [])} main=${JSON.stringify(mainGroups)}`);
  }
  const wasmPath = boot.wasm || (wasmRoot ? wasmRoot + '/game_web_bg.wasm' : '');
  if (!wasmPath || !existsAny(root, wasmPath)) failures.push('boot.wasm 不存在: ' + wasmPath);

  const manifestFile = path.join(root, 'pack.json');
  let manifestEntries = [];
  if (fs.existsSync(manifestFile)) {
    const manifest = readJson(manifestFile);
    manifestEntries = Array.isArray(manifest.entries) ? manifest.entries : [];
    if (JSON.stringify(sorted(boot.entries || [])) !== JSON.stringify(sorted(manifestEntries))) {
      failures.push('boot.entries 与 pack.json.entries 不一致（首屏清单与计划表漂移）');
    }
  } else if ((boot.entries || []).length > 0) {
    failures.push('缺少 pack.json，但 boot.entries 非空');
  }

  const mainRoot = path.join(root, MAIN_ROOT);
  for (const entry of boot.entries || []) {
    const inMain = path.join(mainRoot, entry);
    const inBoot = (boot.roots || []).some((bootRoot) => fs.existsSync(path.join(root, bootRoot, entry)));
    if (!fs.existsSync(inMain) && !inBoot && !existsAny(mainRoot, entry)) {
      failures.push('boot entry 在 pack/ 与 boot roots 里都不存在: ' + entry);
    }
  }
  for (const group of plan.groups.filter((item) => item.unit === 'main')) {
    if (!existsAny(mainRoot, group.bundle)) {
      failures.push(`main 组 ${group.name} 的容器不在 mainpack/: ${group.bundle}`);
    }
  }

  if (fs.existsSync(path.join(root, 'game.json'))) {
    const game = readJson(path.join(root, 'game.json'));
    const roots = (game.subpackages || []).map((item) => String((item && item.root) || '').replace(/\/$/, ''));
    for (const bootRoot of boot.roots || []) {
      if (bootRoot === MAIN_ROOT) continue;
      if (!roots.includes(bootRoot)) failures.push(`game.json 没有声明 boot root: ${bootRoot}`);
    }
  } else {
    failures.push('缺少 game.json');
  }

  let traceChecked = 0;
  if (tracePath) {
    if (!fs.existsSync(tracePath)) throw new Error('找不到 trace 文件: ' + tracePath);
    const trace = readJson(tracePath);
    if (trace.schema !== 1 || !Array.isArray(trace.entries)) throw new Error('trace 格式不认（需要 schema=1 + entries[]）');
    const startupScene = (() => {
      try {
        return fs.existsSync(manifestFile) ? String(readJson(manifestFile).startup_scene || '') : '';
      } catch (error) {
        return '';
      }
    })();
    const bootContexts = new Set(['boot']);
    if (startupScene) bootContexts.add(startupScene);
    const groupOf = (rel) => {
      for (const group of plan.groups) {
        if (group.paths.includes(rel) || group.bundle === rel) return group;
      }
      return null;
    };
    for (const entry of trace.entries) {
      if (!entry || !bootContexts.has(String(entry.context || ''))) continue;
      traceChecked += 1;
      const rel = String(entry.path || '').replace(/^\.\//, '').replace(/^\/+/, '')
        .replace(/^mainpack\//, '').replace(/^pack-wasm\//, '');
      const group = groupOf(rel);
      if (!group) {
        failures.push(`boot trace 路径不在计划表里: ${entry.path}`);
      } else if (group.unit !== 'main') {
        failures.push(`boot trace 读到非 main 单元资源: ${entry.path} -> ${group.name}(${group.unit})`);
      }
    }
  }

  return {
    roots: boot.roots || [],
    groups: boot.groups || [],
    entries: (boot.entries || []).length,
    traceChecked,
    failures,
  };
}

function main() {
  try {
    const args = parseArgs(process.argv);
    const report = checkLauncher(args.launcher, args.trace);
    if (report.failures.length > 0) {
      console.error('boot_closure_failed:\n  - ' + report.failures.join('\n  - '));
      process.exit(1);
    }
    console.log(
      `boot_closure_ok roots=${report.roots.join(',')} groups=${report.groups.join(',')} ` +
      `entries=${report.entries} trace_checked=${report.traceChecked}`,
    );
  } catch (error) {
    console.error('boot_closure_failed: ' + String((error && error.message) || error));
    process.exit(1);
  }
}

if (require.main === module) main();

module.exports = { checkLauncher };
