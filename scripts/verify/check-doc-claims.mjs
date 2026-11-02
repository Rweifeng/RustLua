#!/usr/bin/env node
// 文档声明数字门（P0）：README / docs/README / docs/59 / docs/60 / docs/62 / docs/74 /
// docs/demos 里的 Lua API 与组件计数必须与运行期快照、`ComponentId::ALL` 一致。
// changelog 是历史记录，不在核对范围内。
//
// 为什么需要它：`gen-lua-api-docs.js --check` 守的是"文档桩 == 运行期 API 面"，但不守
// **人写的摘要数字**。2026-10-27 实测 README 仍写 283、docs/74 仍写 301，而快照已是
// 42 表 / 309 函数——两者都能通过各自的门，因为没有任何门读它们。
//
// 口径与其余 node 门一致：判红即非零退出；`--self-test` 用临时 fixture 证明它会红。
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';

const SNAPSHOT_REL = 'engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json';
const COMPONENT_SOURCE_REL = 'engine/crates/rustlua-scene/src/typed_scene.rs';
const SAMPLE_PROJECT_REL = 'engine/crates/engine/src/host/sample_project.rs';

function readText(root, rel) {
  return fs.readFileSync(path.join(root, rel), 'utf8');
}

// README / docs/74 还会写"N 个运行时组件"。事实源是 ComponentId::ALL 的 `Self::` 项数，
// 与 `component_schema_parity` 测试消费的是同一份清单。
export function componentCount(repoRoot) {
  const text = readText(repoRoot, COMPONENT_SOURCE_REL);
  const block = text.match(/pub const ALL[^=]*=\s*&\[([\s\S]*?)\];/);
  if (!block) throw new Error(`${COMPONENT_SOURCE_REL} 里找不到 ComponentId::ALL 定义`);
  const names = [...block[1].matchAll(/\bSelf::([A-Za-z0-9_]+)/g)].map((match) => match[1]);
  if (names.length === 0) throw new Error(`${COMPONENT_SOURCE_REL} 的 ComponentId::ALL 为空`);
  return names.length;
}

export function apiCounts(repoRoot) {
  const snapshot = JSON.parse(readText(repoRoot, SNAPSHOT_REL));
  if (!Array.isArray(snapshot?.tables)) throw new Error(`${SNAPSHOT_REL} 缺少 tables 数组`);
  let funcs = 0;
  for (const table of snapshot.tables) {
    if (!Array.isArray(table?.funcs)) throw new Error(`${SNAPSHOT_REL} 的表 ${table?.name ?? '?'} 缺少 funcs 数组`);
    funcs += table.funcs.length;
  }
  return { tables: snapshot.tables.length, funcs };
}

// `host::sample_project` 的门数是"这个演示工程被多少条测试守着"的事实：README / docs/59
// 都写死了这个数（曾从 11 漂到 12）。只数该文件里以 sample_project 开头的 #[test]。
export function sampleGateCount(repoRoot) {
  const text = readText(repoRoot, SAMPLE_PROJECT_REL);
  const names = [...text.matchAll(/#\[test\]\s*\n\s*fn\s+([A-Za-z0-9_]+)/g)]
    .map((match) => match[1])
    .filter((name) => name.startsWith('sample_project'));
  if (names.length === 0) throw new Error(`${SAMPLE_PROJECT_REL} 里找不到 sample_project_* 测试`);
  return names.length;
}

// 每个 pattern 的捕获组必须是 (tables, funcs)；同一文件里所有命中都要对，避免
// "抬头改对了、正文还留一处旧数"；一处都没命中同样判红（声明被删掉也要重新登记）。
const CLAIMS = [
  {
    file: 'README.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个\s*Lua\s*函数/g,
    label: 'README 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/74-game_api演示工程2.0.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个函数/g,
    label: 'docs/74 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/74-game_api演示工程2.0.md',
    pattern: /（(\d+)\/(\d+)）/g,
    label: 'docs/74 的覆盖率基线（模块表/函数）',
  },
  {
    file: 'docs/74-game_api演示工程2.0.md',
    pattern: /(\d+)\/(\d+)\s*每个函数/g,
    label: 'docs/74 的覆盖率基线（模块表/函数）',
  },
  {
    file: 'docs/59-示例工程game_api与API覆盖率门.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个\s*Lua\s*函数/g,
    label: 'docs/59 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/demos/README.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个函数/g,
    label: 'docs/demos/README 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/README.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个函数/g,
    label: 'docs/README 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/60-全面复盘与优化项清单.md',
    pattern: /(\d+)\s*个模块表\s*\/\s*(\d+)\s*个函数/g,
    label: 'docs/60 的 Lua API 计数（模块表 / 函数）',
  },
  {
    file: 'docs/62-成熟引擎差距分析.md',
    pattern: /(\d+)\s*模块表\s*\/\s*(\d+)\s*绑定函数/g,
    label: 'docs/62 的 Lua API 计数（模块表 / 绑定函数）',
  },
  {
    file: 'docs/62-成熟引擎差距分析.md',
    pattern: /(\d+)\s*模块\s*\/\s*(\d+)\s*函数/g,
    label: 'docs/62 的 API 规模（模块 / 函数）',
  },
];

// 组件计数声明所在文件；changelog 是历史记录，不随当前事实改写。
const COMPONENT_CLAIM_FILES = [
  'README.md',
  'docs/74-game_api演示工程2.0.md',
  'docs/59-示例工程game_api与API覆盖率门.md',
  'docs/demos/README.md',
];

// 文档里写的"代码常量"必须等于源码里的常量。为什么需要：这类声明既不在文档门（只查链接/
// 编号）的射程里，也不在 API 计数门的射程里——`docs/64` 曾是整篇"按计划写"的文档，实现
// （ADR-0023）早已落地而文档停在计划态，正是"实现超前于文档"的典型。留一条可复核的锚点后，
// 改常量不改文档（或反过来）都会立刻判红，而不是等下一个接手者发现。
const CODE_CONSTANTS = [
  {
    label: '脚本连续错误上限 DEFAULT_CALLBACK_ERROR_LIMIT',
    doc: 'docs/64-Lua脚本错误处置策略.md',
    docPattern: /DEFAULT_CALLBACK_ERROR_LIMIT\s*=\s*(\d+)/g,
    source: 'engine/crates/rustlua-scripting/src/script/guard.rs',
    sourcePattern: /DEFAULT_CALLBACK_ERROR_LIMIT\s*:\s*u\d+\s*=\s*(\d+)/,
  },
];
// `tasks/plan/` 的"空/非空"断言与目录事实。为什么需要：README 曾写"`tasks/plan/` 当前为空"，
// 而目录里实际躺着 M78-00（2026-11-02 核对时发现）。这类"状态断言"既不在文档门（只查链接/编号）
// 的射程里，也不在 API 计数门的射程里——没人读它就永远错着。这里把它变成一条可复核的声明。
export function planDirStatus(repoRoot) {
  const dir = path.join(repoRoot, 'tasks', 'plan');
  if (!fs.existsSync(dir)) return { files: 0, exists: false };
  const files = fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith('.md'));
  return { files: files.length, exists: true };
}

export function audit({ repoRoot }) {
  const errors = [];
  let counts;
  let components;
  let gates;
  try {
    counts = apiCounts(repoRoot);
    components = componentCount(repoRoot);
    gates = sampleGateCount(repoRoot);
  } catch (error) {
    return { errors: [`读取事实源失败：${error.message}`], stats: null };
  }
  const { tables, funcs } = counts;
  let checked = 0;
  for (const claim of CLAIMS) {
    const text = readText(repoRoot, claim.file);
    const matches = [...text.matchAll(claim.pattern)];
    if (matches.length === 0) {
      errors.push(`${claim.label}：${claim.file} 里找不到可核对的声明（模式 ${claim.pattern}）`);
      continue;
    }
    for (const match of matches) {
      checked += 1;
      const gotTables = Number(match[1]);
      const gotFuncs = Number(match[2]);
      if (gotTables !== tables || gotFuncs !== funcs) {
        errors.push(
          `${claim.label}：${claim.file} 写 ${gotTables}/${gotFuncs}，快照是 ${tables}/${funcs}`,
        );
      }
    }
  }
  for (const file of COMPONENT_CLAIM_FILES) {
    const text = readText(repoRoot, file);
    const matches = [...text.matchAll(/(\d+)\s*个运行时组件/g)];
    if (matches.length === 0) {
      errors.push(`运行时组件数：${file} 里找不到"N 个运行时组件"声明`);
      continue;
    }
    for (const match of matches) {
      checked += 1;
      if (Number(match[1]) !== components) {
        errors.push(`运行时组件数：${file} 写 ${match[1]}，ComponentId::ALL 是 ${components}`);
      }
    }
  }
  for (const file of ['README.md', 'docs/59-示例工程game_api与API覆盖率门.md']) {
    const text = readText(repoRoot, file);
    const matches = [...text.matchAll(/`host::sample_project` 的 (\d+) 条门/g)];
    if (matches.length === 0) {
      errors.push(`sample_project 门数：${file} 里找不到"\`host::sample_project\` 的 N 条门"声明`);
      continue;
    }
    for (const match of matches) {
      checked += 1;
      if (Number(match[1]) !== gates) {
        errors.push(`sample_project 门数：${file} 写 ${match[1]}，源文件里是 ${gates} 条`);
      }
    }
  }
  for (const claim of CODE_CONSTANTS) {
    const docText = readText(repoRoot, claim.doc);
    const docValues = [...docText.matchAll(claim.docPattern)].map((m) => m[1]);
    if (docValues.length === 0) {
      errors.push(`${claim.label}：${claim.doc} 里找不到可核对的声明（模式 ${claim.docPattern}）`);
      continue;
    }
    const sourceText = readText(repoRoot, claim.source);
    const sourceMatch = claim.sourcePattern.exec(sourceText);
    if (!sourceMatch) {
      errors.push(`${claim.label}：${claim.source} 里找不到常量定义（模式 ${claim.sourcePattern}）`);
      continue;
    }
    for (const value of docValues) {
      checked += 1;
      if (Number(value) !== Number(sourceMatch[1])) {
        errors.push(`${claim.label}：${claim.doc} 写 ${value}，${claim.source} 是 ${sourceMatch[1]}`);
      }
    }
  }  // `tasks/plan/` 状态：README 里的"为空"必须与目录一致。只查"为空"这一种断言——
  // "有几个"这种会变的数字，本仓库的口径是根本不该写进 README（见其「当前状态」的规矩）。
  const plan = planDirStatus(repoRoot);
  for (const match of readText(repoRoot, 'README.md').matchAll(/tasks[\\/]plan[^\n]{0,16}?为空/g)) {
    checked += 1;
    if (plan.files > 0) {
      errors.push(
        `tasks/plan 状态：README 写"${match[0]}"，但 tasks/plan/ 里有 ${plan.files} 个文件`,
      );
    }
  }
  return { errors, stats: { tables, funcs, components, gates, constants: CODE_CONSTANTS.length, planFiles: plan.files, claims: checked } };
}

function writeFixture(root, snapshot, readme, doc74) {
  const files = {
    [SNAPSHOT_REL]: JSON.stringify(snapshot),
    [COMPONENT_SOURCE_REL]: 'pub const ALL: &\'static [Self] = &[\n    Self::Transform,\n    Self::Camera,\n];\n',
    [SAMPLE_PROJECT_REL]: '#[test]\nfn sample_project_alpha() {}\n#[test]\nfn sample_project_beta() {}\n',
    'README.md': readme,
    'docs/74-game_api演示工程2.0.md': doc74,
    'docs/59-示例工程game_api与API覆盖率门.md': '活文档：**2 个模块表 / 3 个 Lua 函数**、2 个运行时组件，覆盖度由 `host::sample_project` 的 2 条门守。\n',
    'docs/demos/README.md': '| `examples/game_api` | 2 个模块表 / 3 个函数 / 2 个运行时组件 |\n',
    'docs/README.md': '- 演示工程 2.0：2 个模块表 / 3 个函数\n',
    'docs/60-全面复盘与优化项清单.md': '| Lua API | 2 个模块表 / 3 个函数 |\n',
    'docs/62-成熟引擎差距分析.md': '| 脚本 | **2 模块表 / 3 绑定函数** |\n| API 规模 | 2 模块 / 3 函数 |\n',
    'docs/64-Lua脚本错误处置策略.md': '上限 DEFAULT_CALLBACK_ERROR_LIMIT = 60。\n',
    'engine/crates/rustlua-scripting/src/script/guard.rs': 'pub const DEFAULT_CALLBACK_ERROR_LIMIT: u32 = 60;\n',
  };
  for (const [rel, text] of Object.entries(files)) {
    const abs = path.join(root, rel);
    fs.mkdirSync(path.dirname(abs), { recursive: true });
    fs.writeFileSync(abs, text);
  }
}

function selfTest() {
  const snapshot = {
    tables: [
      { name: 'alpha', funcs: [{ name: 'a' }, { name: 'b' }] },
      { name: 'beta', funcs: [{ name: 'c' }] },
    ],
  };
  const goodReadme = '快速入口：**2 个模块表 / 3 个 Lua 函数**，2 个运行时组件全部有演示，覆盖度由 `host::sample_project` 的 2 条门守。\n';
  const goodDoc = [
    '> 基线：**2 个模块表 / 3 个函数**；',
    '> **2 个运行时组件**全部有演示；',
    '| 覆盖率 | 门 | 2/3 每个函数都有调用点 |',
    '| 4 | 覆盖率 | （2/3） |',
  ].join('\n');
  const cases = [
    ['一致为绿', snapshot, goodReadme, goodDoc, false],
    ['README 旧数红', snapshot, goodReadme.replace('3 个 Lua 函数', '9 个 Lua 函数'), goodDoc, true],
    ['docs 抬头旧数红', snapshot, goodReadme, goodDoc.replace('2 个模块表 / 3 个函数', '2 个模块表 / 9 个函数'), true],
    ['docs 正文漏改红', snapshot, goodReadme, goodDoc.replace('2/3 每个函数', '2/9 每个函数'), true],
    ['组件数漂移红', snapshot, goodReadme.replace('2 个运行时组件', '9 个运行时组件'), goodDoc, true],
    ['门数漂移红', snapshot, goodReadme.replace('2 条门', '9 条门'), goodDoc, true],
    ['快照坏红', { tables: [{ name: 'alpha' }] }, goodReadme, goodDoc, true],
  ];
  let failed = 0;
  for (const [name, snapshotCase, readme, doc74, wantRed] of cases) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'doc-claims-'));
    writeFixture(dir, snapshotCase, readme, doc74);
    const { errors } = audit({ repoRoot: dir });
    const gotRed = errors.length > 0;
    if (gotRed !== wantRed) {
      failed += 1;
      console.error(`自测失败：${name} 期望 ${wantRed ? '红' : '绿'}，实际 ${gotRed ? '红' : '绿'} ${errors.join(' | ')}`);
    }
    fs.rmSync(dir, { recursive: true, force: true });
  }
  if (failed > 0) process.exit(1);

  // tasks/plan 状态断言：目录非空而 README 写"为空"必须判红。
  // 正向（目录为空 + 同样的句子）由上面各例隐式覆盖：它们既没有该句、也没有 tasks/plan 目录。
  {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'doc-claims-plan-'));
    writeFixture(dir, snapshot, `${goodReadme}\`tasks/plan/\` 当前为空。\n`, goodDoc);
    fs.mkdirSync(path.join(dir, 'tasks', 'plan'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'tasks', 'plan', 'M78-00-x.md'), '# x\n');
    const { errors } = audit({ repoRoot: dir });
    if (errors.length === 0) {
      failed += 1;
      console.error('自测失败：tasks/plan 非空但 README 声称"为空"未判红');
    }
    fs.rmSync(dir, { recursive: true, force: true });
    if (failed > 0) process.exit(1);
  }

  // 代码常量锚点：文档写的常量与源码不一致必须判红（docs/64 的"实现超前于文档"就是这类漂移）。
  {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'doc-claims-const-'));
    writeFixture(dir, snapshot, goodReadme, goodDoc);
    fs.writeFileSync(
      path.join(dir, 'docs/64-Lua脚本错误处置策略.md'),
      '上限 `DEFAULT_CALLBACK_ERROR_LIMIT = 99`。\n',
    );
    const { errors } = audit({ repoRoot: dir });
    if (errors.length === 0) {
      failed += 1;
      console.error('自测失败：docs/64 的常量与源码不一致未判红');
    }
    fs.rmSync(dir, { recursive: true, force: true });
    if (failed > 0) process.exit(1);
  }

  console.log(`doc_claims_selftest_ok cases=${cases.length + 2}`);
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const repoRoot = path.resolve(import.meta.dirname, '..', '..');
  const { errors, stats } = audit({ repoRoot });
  for (const error of errors) console.error('doc_claims: ' + error);
  if (errors.length > 0) process.exit(1);
  console.log(
    `doc_claims_ok tables=${stats.tables} funcs=${stats.funcs} components=${stats.components} gates=${stats.gates} plan_files=${stats.planFiles} code_consts=${stats.constants} claims=${stats.claims}`,
  );
}
