// Web↔PC API 一致性门 · L2「覆盖清单」检查（scripts/check-web-api-overrides.mjs）
//
// 要钉住的漂移：`web-runtime.js` 里那批 Lua 宿主补丁**不是在补齐**，而是在**覆盖**引擎
// 已经注册好的实现。实测（docs/51 记的"按引擎源码逐条实现宿主 API"就是这一层）：
//   补丁层定义点 57 个 → 覆盖 57 / 补齐 0 / 未知表名 0（占 275 个 API 的 21%）。
// 名字面两边完全对得上，所以任何"只比名字"的门都抓不到这里的分叉——分叉在行为：
// 例如 `clock.now()` 引擎版返回 day/minute/hour/minute_of_day/rate/paused，
// 补丁版只返回 day/minute/minute_of_day（实测），Lua 脚本读到 nil 而不报错。
//
// 本脚本做的是**清单层**的自动化：把补丁层重定义的「模块.函数」全部抽出来，与提交进仓库的
// `plugins/webgl/lua_api_overrides.json` 对拍。新增一个覆盖点必须显式进这张表并写理由，
// 否则红；删掉一个覆盖点（比如把 shim 换回引擎实现）也必须同步表，否则同样红。
//
// 判据不依赖构建产物：分类用的注册层名字集合来自提交在仓库里的桌面基线
// （`api_surface_snapshot/lua_api_surface.json`），L1 门再锁"wasm 活体 == 那份基线"。
//
// 用法：
//   node scripts/check-web-api-overrides.mjs          # 检查（exit 0/1）
//   node scripts/check-web-api-overrides.mjs --write  # 重生成清单（保留已有理由，新增留空待补）
//
// 退出码：0 通过；1 不一致（含新覆盖点没写理由）；3 输入文件缺失（环境/路径问题）。
'use strict';

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const runtimePath = join(repoRoot, 'plugins/webgl/web-runtime.js');
const baselinePath = join(
  repoRoot,
  'engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json'
);
const overridesPath = join(repoRoot, 'plugins/webgl/lua_api_overrides.json');

const write = process.argv.includes('--write');

for (const [label, file] of [['web-runtime.js', runtimePath], ['桌面 API 基线', baselinePath], ['覆盖清单', overridesPath]]) {
  if (!existsSync(file)) {
    console.error(`缺少输入文件（${label}）: ${file}`);
    process.exitCode = 3;
    throw new Error('input missing');
  }
}

const source = readFileSync(runtimePath, 'utf8');
const baseline = JSON.parse(readFileSync(baselinePath, 'utf8'));
const registered = new Map(baseline.tables.map((table) => [table.name, new Set(table.funcs)]));

// 补丁层的定义点：Lua 里 `function a.b(` 与 `a.b = function` 两种写法。
// 锚在行首——Lua 补丁串都是顶格写的，JS 侧代码不会误伤（JS 的 `window.x = function`
// 里的 `x` 也不是宿主模块表名，下一步分类会滤掉）。
const patterns = [
  /^function\s+([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm,
  /^([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*=\s*function/gm,
];
const live = new Map();
for (const pattern of patterns) {
  let match;
  while ((match = pattern.exec(source)) !== null) {
    const [, table, func] = match;
    if (!registered.has(table)) continue;
    const path = `${table}.${func}`;
    if (!live.has(path)) live.set(path, source.slice(0, match.index).split('\n').length);
  }
}

const committed = JSON.parse(readFileSync(overridesPath, 'utf8'));
const committedByApi = new Map();
for (const group of committed.groups) {
  for (const api of group.apis) {
    if (committedByApi.has(api)) {
      console.error(`清单里 ${api} 出现在多个分组（一个覆盖点只能归一处）`);
      process.exitCode = 1;
    }
    committedByApi.set(api, group);
  }
}

// 分类：覆盖（引擎已注册同名）vs 补齐（引擎没有）。补齐不是本门的对象，但**必须为 0**——
// 出现补齐说明补丁层在发明 PC 没有的 API，那 Lua 脚本就能只在 web 上跑起来。
const phantom = [...live.keys()].filter((api) => !committedByApi.has(api));
const stale = [...committedByApi.keys()].filter((api) => !live.has(api));
// 引擎注册层里"有这个名字"是覆盖的定义；不在注册层的定义点在上面已被 continue 掉，
// 这里再明确查一次，确保没人拿补丁发明新 API。
const additions = [...live.keys()].filter((api) => !registered.get(api.split('.')[0]).has(api.split('.')[1]));

if (write) {
  // 保留已有理由；新出现的覆盖点归到其模块分组并留空 note，由作者补。
  const groups = committed.groups.map((group) => ({ ...group, apis: group.apis.filter((api) => live.has(api)).sort() }));
  const knownModules = new Set(groups.map((group) => group.module));
  for (const api of [...live.keys()].sort()) {
    if (groups.some((group) => group.apis.includes(api))) continue;
    const module = api.split('.')[0];
    if (knownModules.has(module)) {
      groups.find((group) => group.module === module).apis.push(api);
    } else {
      groups.push({ module, note: '', apis: [api] });
      knownModules.add(module);
    }
  }
  groups.sort((a, b) => (a.module < b.module ? -1 : 1));
  for (const group of groups) group.apis.sort();
  writeFileSync(overridesPath, JSON.stringify({ groups }, null, 2) + '\n');
  console.log(`已重生成 ${overridesPath}：${live.size} 个覆盖点`);
  process.exitCode = 0;
} else {
  const missingNote = committed.groups.filter((group) => !String(group.note || '').trim());
  const issues = [];
  if (phantom.length) {
    issues.push(`补丁层新增了 ${phantom.length} 个未登记的覆盖点: ${phantom.join(', ')}`
      + `\n  → 跑 node scripts/check-web-api-overrides.mjs --write，并给所在分组补上理由`
      + '\n  → 理由要写清"为什么 web 必须自己实现"，以及"行为与引擎版差在哪"');
  }
  if (stale.length) {
    issues.push(`清单里有 ${stale.length} 个补丁层已不再定义的覆盖点: ${stale.join(', ')}`
      + '（shim 删了就要把条目一起删，否则清单不再描述真实代码）');
  }
  if (additions.length) {
    issues.push(`补丁层定义了 ${additions.length} 个桌面注册层没有的 API: ${additions.join(', ')}`
      + '（那是"只在 web 存在的幽灵 API"，游戏脚本会写出无法回桌面的调用）');
  }
  if (missingNote.length) {
    issues.push(`${missingNote.length} 个分组没有理由: ${missingNote.map((g) => g.module).join(', ')}`
      + '（每个覆盖分组必须说明为什么 web 自己实现）');
  }
  if (issues.length) {
    console.error(`web API 覆盖清单与代码不一致（桌面基线 ${baseline.tables.length} 张表 / 补丁层覆盖点 ${live.size} 个）：`);
    for (const issue of issues) console.error('  - ' + issue);
    process.exitCode = 1;
  } else {
    console.log(`web_api_overrides_ok overrides=${live.size} modules=${committed.groups.length}`
      + `（覆盖点与清单一致，且无幽灵 API）`);
    process.exitCode = 0;
  }
}
