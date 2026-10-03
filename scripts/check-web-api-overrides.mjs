// Web↔PC API 一致性门 · L2「覆盖清单」检查（scripts/check-web-api-overrides.mjs）
//
// 要钉住的漂移：`web-runtime.js` 里那批 Lua 宿主补丁**不是在补齐**，而是在**覆盖**引擎
// 已经注册好的实现。实测（docs/51 记的"按引擎源码逐条实现宿主 API"就是这一层）：
//   补丁层函数式定义点 56 个 → 覆盖 56 / 补齐 0 / 未知表名 0（占 275 个 API 的 20%）。
// 名字面两边完全对得上，所以任何"只比名字"的门都抓不到这里的分叉——分叉在行为：
// 例如 `clock.now()` 引擎版返回 day/minute/hour/minute_of_day/rate/paused，
// 补丁版只返回 day/minute/minute_of_day（实测），Lua 脚本读到 nil 而不报错。
//
// 本脚本做两层清单自动化：
//
// 一、**覆盖点清单**（老功能）：把补丁层以 `function a.b(` / `a.b = function` 重定义的
// 「模块.函数」全部抽出来，与 `plugins/webgl/lua_api_overrides.json` 的 `groups` 对拍。
// 新增一个覆盖点必须显式进表并写理由，删掉一个必须同步删条目，否则红。
//
// 二、**整表替换账**（防复发的关键）：补丁层还有第三种写法——`input = { … }` 这种
// **把整张引擎表换掉**的赋值。它不是"覆盖某个函数"，而是"这张表里没重写的函数全没了"，
// 而 Lua 读到 nil **不报错**（只在调用那一刻炸，或静静走 falsy 分支）。实测 23 张表这么写、
// 引擎侧 275 个函数里 77 个因此消失。上面的抽取式（`function a.b(` 与 `a.b = function`）
// 对这种写法**完全瞎**，所以这一层单独抽：
//   1) 站点：列 0 锚定的 `^<引擎注册表名> = {`（补丁串里 Lua 顶层语句都顶格；不锚列 0
//      会误伤 JS 侧的 `audio = {` 赋值）；
//   2) 表体：括号深度 + Lua 块层级（function/if/do 进、end 出）+ 字符串与注释跳过，
//      只取**顶层键**——不记块层级就会把函数体里的 `local value = {}` 当成表键（实测踩过）；
//   3) 第三类藏得更深的定义点：`for _, api in ipairs({audio, input}) do api.load_settings = …`
//      （循环变量批量补函数，前两种抽取式都看不见它）；
//   4) 与清单对拍 `dropped`（引擎有、补丁没提供＝在 web 上是 nil）、`invented`（补丁发明、
//      引擎没有＝只在 web 跑得通的幽灵 API）、以及 `referenced`（dropped 里被发包工程脚本
//      引用的那些＝真会撞上 nil 的那部分）。三个列表任一不符⇒红；登记了但站点已消失⇒红。
//
// `referenced` 的口径：只扫 `examples/*/scripts/**/*.lua`（发包工程的源码），**先剥掉 Lua
// 注释与字符串字面量**——不剥的话 `log.warn("[map] map.goto 失败")` 这种文案会被算成调用
// （实测虚报 14 → 剥后 9）；`examples/*/builds/**` 是发包产物，同样不参与扫描。
//
// 判据不依赖构建产物：分类用的注册层名字集合来自提交在仓库里的桌面基线
// （`api_surface_snapshot/lua_api_surface.json`），L1 门再锁"wasm 活体 == 那份基线"。
//
// 用法：
//   node scripts/check-web-api-overrides.mjs          # 检查（exit 0/1）
//   node scripts/check-web-api-overrides.mjs --write  # 重生成清单（保留已有理由，新增留空待补）
//
// 退出码：0 通过；1 不一致（含新覆盖点/新整表替换点没写理由）；3 输入文件缺失。
'use strict';

import { readFileSync, writeFileSync, existsSync, readdirSync, statSync } from 'node:fs';
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

// 扫一个 Lua table constructor：from 指向起始 '{'，返回顶层键与闭合位置。
// 三层记账：括号深度、Lua 块层级、字符串/注释跳过。
function scanLuaTable(text, from) {
  const keys = new Set();
  let i = from;
  let depth = 0;
  let blocks = 0;
  let buf = '';
  const flush = () => {
    const name = buf.trim();
    buf = '';
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return;
    if (depth === 1 && blocks === 0 && /^\s*=/.test(text.slice(i + 1, i + 31))) keys.add(name);
  };
  // 关键字匹配必须**锚在当前位置**：re.exec(text.slice(i)) 不带 ^ 会在串里任意位置命中，
  // 然后 i 跳到无关偏移，把中间的代码整段吃掉（实测所有键都扫不出来）。
  const keyword = (name) => {
    if (i > 0 && /\w/.test(text[i - 1])) return false;
    if (!text.startsWith(name, i)) return false;
    const next = text[i + name.length];
    if (next !== undefined && /\w/.test(next)) return false;
    i += name.length;
    return true;
  };
  while (i < text.length) {
    const c = text[i];
    const two = text.slice(i, i + 2);
    if (two === '--') {
      const nl = text.indexOf('\n', i);
      if (text.slice(i + 2, i + 4) === '[[') {
        const close = text.indexOf(']]', i + 4);
        i = close < 0 ? text.length : close + 2;
      } else i = nl < 0 ? text.length : nl;
      buf = '';
      continue;
    }
    if (two === '[[') {
      const close = text.indexOf(']]', i + 2);
      i = close < 0 ? text.length : close + 2;
      continue;
    }
    if (c === "'" || c === '"') {
      let j = i + 1;
      while (j < text.length) {
        if (text[j] === '\\') { j += 2; continue; }
        if (text[j] === c) break;
        j += 1;
      }
      i = j + 1;
      buf = '';
      continue;
    }
    if (keyword('function') || keyword('if') || keyword('do')) { blocks += 1; buf = ''; continue; }
    if (keyword('end')) { blocks -= 1; buf = ''; continue; }
    if (c === '{' || c === '(') { depth += 1; buf = ''; i += 1; continue; }
    if (c === '}' || c === ')') {
      depth -= 1;
      if (depth === 0) return { keys, end: i };
      buf = '';
      i += 1;
      continue;
    }
    if (c === ',' || c === ';') { buf = ''; i += 1; continue; }
    buf += c;
    if (/[\s=]/.test(c) && depth === 1 && buf.trim() !== '') flush();
    i += 1;
  }
  return { keys, end: i };
}

// （一）函数式覆盖点：`function a.b(` 与 `a.b = function`，锚在行首。
const funcPatterns = [
  /^function\s+([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm,
  /^([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*=\s*function/gm,
];
const live = new Map();
const funcDefs = new Map();
for (const pattern of funcPatterns) {
  let match;
  while ((match = pattern.exec(source)) !== null) {
    const [, table, func] = match;
    if (!registered.has(table)) continue;
    const path = `${table}.${func}`;
    if (!live.has(path)) live.set(path, source.slice(0, match.index).split('\n').length);
    if (!funcDefs.has(table)) funcDefs.set(table, new Set());
    funcDefs.get(table).add(func);
  }
}

// （二·1）整表替换站点：列 0 锚定 `^<注册表名> = {`。
const sitePattern = /^([A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*\{/gm;
const sites = [];
const literalKeys = new Map();
let siteMatch;
while ((siteMatch = sitePattern.exec(source)) !== null) {
  const table = siteMatch[1];
  if (!registered.has(table)) continue;
  const braceIndex = siteMatch.index + siteMatch[0].length - 1;
  const { keys, end } = scanLuaTable(source, braceIndex);
  const line = source.slice(0, siteMatch.index).split('\n').length;
  sites.push({ table, line, keys: [...keys].sort(), body: source.slice(braceIndex, end) });
  if (!literalKeys.has(table)) literalKeys.set(table, new Set());
  for (const key of keys) literalKeys.get(table).add(key);
}

// （二·2）循环批量补函数：`for _, api in ipairs({audio, input}) do … api.x = function`。
// 这类写法给多张表同时补同一个名字，抽取式必须跟着 ipairs 的表清单走。
const loopPattern = /^for[ \t]+([A-Za-z_][A-Za-z0-9_]*(?:[ \t]*,[ \t]*[A-Za-z_][A-Za-z0-9_]*)*)[ \t]+in[ \t]+ipairs\([ \t]*\{([^}]*)\}[ \t]*\)[ \t]*do/gm;
const loopKeys = new Map();
let loopMatch;
while ((loopMatch = loopPattern.exec(source)) !== null) {
  const loopVars = loopMatch[1].split(',').map((name) => name.trim()).filter(Boolean);
  const loopTables = loopMatch[2].split(',').map((name) => name.trim()).filter((name) => registered.has(name));
  if (!loopTables.length) continue;
  const bodyStart = loopMatch.index + loopMatch[0].length;
  const rest = source.slice(bodyStart);
  const bodyEnd = rest.search(/^end\b/m);
  const body = rest.slice(0, bodyEnd < 0 ? 4000 : bodyEnd);
  for (const variable of loopVars) {
    const assign = new RegExp(String.raw`\b${variable}\.([A-Za-z_][A-Za-z0-9_]*)[ \t]*=`, 'g');
    let assignMatch;
    while ((assignMatch = assign.exec(body)) !== null) {
      for (const table of loopTables) {
        if (!loopKeys.has(table)) loopKeys.set(table, new Set());
        loopKeys.get(table).add(assignMatch[1]);
      }
    }
  }
}

const providedOf = (table) => new Set([
  ...(literalKeys.get(table) ?? []),
  ...(funcDefs.get(table) ?? []),
  ...(loopKeys.get(table) ?? []),
]);

// 发包工程的 Lua 源码（剥注释与字符串），用来算 dropped 里"真的被引用"的那部分。
// 口径：只扫 `examples/*/scripts/**/*.lua`，跳过 `builds/`（发包产物，不是源码）。
// `game_api` 是**按设计把每个 API 都调一遍**的展示 demo，它引不引用不构成风险信号，
// 所以从引用扫描里排除——排除后 referenced 读的就是"真会在 web 上取到 nil"的那几个名字
// （2026-10-03 实测：含 game_api 时 76 个，排除后 9 个，全在 game_jhyt）。名单见
// REFERENCE_SCAN_SKIP。
const stripLuaNoise = (text) => {
  let out = '';
  let i = 0;
  while (i < text.length) {
    const two = text.slice(i, i + 2);
    if (two === '--') {
      if (text.slice(i + 2, i + 4) === '[[') {
        const close = text.indexOf(']]', i + 4);
        i = close < 0 ? text.length : close + 2;
      } else {
        const nl = text.indexOf('\n', i);
        i = nl < 0 ? text.length : nl;
      }
      continue;
    }
    if (two === '[[') {
      const close = text.indexOf(']]', i + 2);
      i = close < 0 ? text.length : close + 2;
      continue;
    }
    if (text[i] === "'" || text[i] === '"') {
      const quote = text[i];
      let j = i + 1;
      while (j < text.length) {
        if (text[j] === '\\') { j += 2; continue; }
        if (text[j] === quote) break;
        j += 1;
      }
      i = j + 1;
      continue;
    }
    out += text[i];
    i += 1;
  }
  return out;
};
const scriptDirs = existsSync(join(repoRoot, 'examples')) ? readdirSync(join(repoRoot, 'examples')) : [];
const REFERENCE_SCAN_SKIP = new Set(['game_api']);
const scriptFiles = [];
const collectLua = (dir) => {
  for (const entry of readdirSync(dir)) {
    if (entry === 'builds' || entry === 'target' || entry === 'node_modules') continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) collectLua(full);
    else if (entry.endsWith('.lua')) scriptFiles.push(full);
  }
};
for (const project of scriptDirs) {
  if (REFERENCE_SCAN_SKIP.has(project)) continue;
  const scripts = join(repoRoot, 'examples', project, 'scripts');
  if (existsSync(scripts)) collectLua(scripts);
}
const gameSources = stripLuaNoise(scriptFiles.map((file) => readFileSync(file, 'utf8')).join('\n'));
const referencedBy = (table, names) => names.filter((name) =>
  new RegExp(String.raw`\b${table}\.${name}\b`).test(gameSources)
);

const wholesaleTables = [...new Set(sites.map((site) => site.table))].sort();
const computed = new Map();
for (const table of wholesaleTables) {
  const provided = providedOf(table);
  const dropped = [...registered.get(table)].filter((name) => !provided.has(name)).sort();
  computed.set(table, {
    dropped,
    invented: [...provided].filter((name) => !registered.get(table).has(name)).sort(),
    referenced: referencedBy(table, dropped),
    line: sites.find((site) => site.table === table).line,
  });
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
const committedReplacements = new Map(
  (committed.replacements ?? []).map((entry) => [entry.module, entry])
);

const phantom = [...live.keys()].filter((api) => !committedByApi.has(api));
const stale = [...committedByApi.keys()].filter((api) => !live.has(api));
const additions = [...live.keys()].filter((api) => !registered.get(api.split('.')[0]).has(api.split('.')[1]));
const unregisteredSites = wholesaleTables.filter((table) => !committedReplacements.has(table));
const staleSites = [...committedReplacements.keys()].filter((table) => !computed.has(table));
const drift = [];
for (const [table, entry] of committedReplacements) {
  const now = computed.get(table);
  if (!now) continue;
  for (const field of ['dropped', 'invented', 'referenced']) {
    const before = [...(entry[field] ?? [])].sort();
    const after = now[field];
    const added = after.filter((name) => !before.includes(name));
    const gone = before.filter((name) => !after.includes(name));
    if (added.length || gone.length) {
      drift.push(`${table}.${field}`
        + (added.length ? ` 新增 [${added.join(', ')}]` : '')
        + (gone.length ? ` 已消失 [${gone.join(', ')}]` : ''));
    }
  }
}

const totals = (field) => wholesaleTables.reduce((sum, table) => sum + computed.get(table)[field].length, 0);

if (write) {
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

  // 整表替换账：理由按模块保留，三个列表一律按当前代码重算（新站点的 note 留空待补）。
  const replacements = wholesaleTables.map((table) => ({
    module: table,
    note: committedReplacements.get(table)?.note ?? '',
    ...computed.get(table),
  }));
  for (const [table, entry] of committedReplacements) {
    if (!replacements.some((item) => item.module === table)) {
      console.error(`提示：清单里登记的 ${table} 已不再被整表替换，本次重生成已把它移除`);
    }
  }
  writeFileSync(overridesPath, JSON.stringify({ groups, replacements }, null, 2) + '\n');
  console.log(`已重生成 ${overridesPath}：${live.size} 个覆盖点 / ${replacements.length} 处整表替换`
    + `（dropped=${totals('dropped')} invented=${totals('invented')} referenced=${totals('referenced')}）`);
  process.exitCode = 0;
} else {
  const missingNote = committed.groups.filter((group) => !String(group.note || '').trim());
  const missingReplaceNote = (committed.replacements ?? [])
    .filter((entry) => !String(entry.note || '').trim())
    .map((entry) => entry.module);
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
  if (unregisteredSites.length) {
    issues.push(`补丁层整表替换了 ${unregisteredSites.length} 张引擎表但账上没有: ${unregisteredSites.join(', ')}`
      + `\n  → 这类写法把表里没重写的引擎函数**全删了**，脚本读到的是 nil 而不是报错`
      + `\n  → 跑 node scripts/check-web-api-overrides.mjs --write 生成 dropped/invented/referenced，`
      + '并给每张表补 note 说明"为什么必须换表"与"丢掉的那些在 web 上怎么办"');
  }
  if (staleSites.length) {
    issues.push(`账上记着 ${staleSites.length} 处已经不存在的整表替换: ${staleSites.join(', ')}`
      + '（补丁改回了增量覆盖，就要把条目一起删，否则账不再描述真实代码）');
  }
  if (drift.length) {
    issues.push(`${drift.length} 处整表替换的账与代码不符：`);
    for (const line of drift) issues.push(`    · ${line}`);
    issues.push('  → dropped 变了＝补丁层悄悄增删了引擎函数的有无；referenced 变了＝发包工程'
      + '开始（或不再）引用一个 web 上不存在的名字，两种都必须显式过账');
  }
  if (missingNote.length) {
    issues.push(`${missingNote.length} 个分组没有理由: ${missingNote.map((g) => g.module).join(', ')}`
      + '（每个覆盖分组必须说明为什么 web 自己实现）');
  }
  if (missingReplaceNote.length) {
    issues.push(`${missingReplaceNote.length} 处整表替换没有理由: ${missingReplaceNote.join(', ')}`
      + '（要写清为什么必须换整张表，以及被丢掉的引擎函数在 web 上怎么处理）');
  }
  if (issues.length) {
    console.error(`web API 覆盖清单与代码不一致（桌面基线 ${baseline.tables.length} 张表 /`
      + ` 覆盖点 ${live.size} 个 / 整表替换 ${wholesaleTables.length} 处）：`);
    for (const issue of issues) console.error('  - ' + issue);
    process.exitCode = 1;
  } else {
    const referenced = totals('referenced');
    console.log(`web_api_overrides_ok overrides=${live.size} modules=${committed.groups.length}`
      + ` wholesale=${wholesaleTables.length} dropped=${totals('dropped')}`
      + ` invented=${totals('invented')} referenced=${referenced}`
      + `（覆盖点、整表替换账与代码一致，且无幽灵 API）`);
    if (referenced) {
      console.log(`  其中 ${referenced} 个被丢掉的函数**发包工程正在引用**（web 上取到 nil），`
        + '逐条见 plugins/webgl/lua_api_overrides.json 的 referenced 与 docs/51');
    }
    process.exitCode = 0;
  }
}
