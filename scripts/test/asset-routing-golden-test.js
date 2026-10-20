#!/usr/bin/env node
// 跨端路径黄金对拍（JS 侧）——与 `engine/crates/rustlua-infra/tests/asset_routing_golden.rs`
// 读**同一份** `scripts/test/fixtures/asset-routing-golden.json`。
//
// 三个读者：Rust（规范化/组归属/布局）、JS（本文件：rootOfUnit/planRootIndex/rootForPath）、
// 发布产物（`assetc closure`）。各自的单测都能绿，而它们对同一条路径给出不同答案时，
// 症状是真机白图：容器里有字节，读取端拿另一个键去查。
//
// 用法：node scripts/test/asset-routing-golden-test.js

'use strict';

const fs = require('fs');
const path = require('path');

const rootsModule = require(path.resolve(__dirname, '../../plugins/wechat-minigame/bundle-roots.js'));
const FIXTURE = path.join(__dirname, 'fixtures', 'asset-routing-golden.json');

function fail(message) {
  throw new Error(message);
}

function main() {
  const golden = JSON.parse(fs.readFileSync(FIXTURE, 'utf8').replace(/^\uFEFF/, ''));
  if (golden.schema !== 1) fail('黄金表 schema 不认识: ' + golden.schema);
  // 与 Rust 侧同一判据：这张计划表必须能被真正的解析器接受（结构校验含 paths 排序与唯一归属）。
  const plan = rootsModule.parsePlan(JSON.stringify(golden.plan));
  const index = rootsModule.planRootIndex(plan);

  let checks = 0;
  for (const c of golden.units) {
    const actual = rootsModule.rootOfUnit(c.unit);
    if (actual !== c.root) fail(`单元 ${c.unit} 的根不一致: js=${actual} golden=${c.root}`);
    checks += 1;
  }

  for (const c of golden.paths) {
    const actual = rootsModule.rootForPath(index, c.path);
    if (actual !== c.root) fail(`路径 ${c.path} 的根不一致: js=${actual} golden=${c.root}`);
    // cdn 的路径不进挂载表，但必须"知道它存在且不进包"——`planRootIndex` 直接跳过 cdn 组，
    // 所以这里两者的可观察行为都是 null。差异在于 plan 里确实声明了它，故再查一遍归属。
    const owner = plan.groups.filter((g) => g.paths.includes(c.path));
    if (owner.length !== 1 || owner[0].name !== c.group) {
      fail(`路径 ${c.path} 的归属组不一致: js=${owner.map((g) => g.name).join(',')} golden=${c.group}`);
    }
    if (owner[0].unit !== c.unit) {
      fail(`路径 ${c.path} 的 unit 不一致: js=${owner[0].unit} golden=${c.unit}`);
    }
    checks += 1;
  }

  for (const c of golden.containers) {
    const actual = rootsModule.rootForPath(index, c.path);
    if (actual !== c.root) fail(`容器 ${c.path} 的根不一致: js=${actual} golden=${c.root}`);
    checks += 1;
  }

  // wasm 不是资源组，但适配层只问 `rootForPath('game_web_bg.wasm')`。
  const wasmRoot = plan.wasm_root;
  const wasmActual = rootsModule.rootForPath(index, 'game_web_bg.wasm');
  if (wasmActual !== wasmRoot) {
    fail(`wasm 的根不一致: js=${wasmActual} plan.wasm_root=${wasmRoot}`);
  }
  checks += 1;

  // 计划外路径：单个路径查询返回 null（调用方决定回退），批量接口则必须响亮失败。
  if (rootsModule.rootForPath(index, 'assets/not-planned.png') !== null) {
    fail('计划外路径不该被路由到任何根');
  }
  let threw = false;
  try {
    rootsModule.rootsForPaths(index, ['assets/not-planned.png']);
  } catch (error) {
    threw = true;
  }
  if (!threw) fail('rootsForPaths 对计划外路径必须抛错');
  checks += 2;

  // 每个根需要哪些非主包根：按黄金表分组求，避免"一次性挂全部"。
  const expectedRoots = {};
  for (const c of golden.paths) {
    if (c.root === null || c.root === 'pack') continue;
    expectedRoots[c.root] = (expectedRoots[c.root] || 0) + 1;
  }
  const groupsByRoot = new Map();
  for (const c of golden.paths) {
    if (c.root === null || c.root === 'pack') continue;
    if (!groupsByRoot.has(c.group)) groupsByRoot.set(c.group, []);
    groupsByRoot.get(c.group).push(c.path);
  }
  for (const [groupName, paths] of groupsByRoot) {
    const needed = rootsModule.rootsForPaths(index, paths);
    const root = expectedRootsForGroup(golden, groupName);
    if (needed.length !== 1 || needed[0] !== root) {
      fail(`组 ${groupName} 需要挂载的根不一致: js=${needed.join(',')} golden=${root}`);
    }
    checks += 1;
  }

  // 非法 unit：JS 侧必须抛错，而不是静默回落到主包（回落的后果是分包资源被当主包读）。
  for (const unit of ['', 'sub:', 'sub:a:b', 'Sub', 'main:x']) {
    let bad = false;
    try {
      rootsModule.rootOfUnit(unit);
    } catch (error) {
      bad = true;
    }
    if (!bad) fail('非法 unit 必须抛错: ' + JSON.stringify(unit));
    checks += 1;
  }

  console.log(
    `asset_routing_golden_ok units=${golden.units.length} paths=${golden.paths.length} ` +
    `containers=${golden.containers.length} checks=${checks}`,
  );
}

function expectedRootsForGroup(golden, groupName) {
  for (const c of golden.paths) {
    if (c.group === groupName) return c.root;
  }
  throw new Error('黄金表里没有组: ' + groupName);
}

try {
  main();
} catch (error) {
  console.error('asset_routing_golden_failed: ' + String((error && error.message) || error));
  process.exit(1);
}
