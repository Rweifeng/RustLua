// Lua API 文档桩生成器（docs/61）
//
// 做什么：从**运行期 API 面基线** `lua_api_surface.json` 出发，为每个 Lua 模块表生成一节
// markdown 文档桩（每个绑定函数一个小节，含从 Rust 绑定源码推断出的 Lua 侧签名），并生成
// `engine::lua_api::<模块>` 这个纯文档模块把它们挂进 rustdoc——发布站上可以直接按模块浏览。
//
// 为什么事实源是基线而不是源码：运行期真正暴露的函数清单由快照门（`core::api_surface_snapshot`）
// 钉成基线；而源码里的注册点有 ①直写 ②宏展开 ③循环注册 三种形状（见下）。以基线为主、源码为辅，
// 才能同时发现两类失真：
//   - 「基线有、源码定位不到」→ 注册形状变了，本解析器该修（不会静默降级成没签名）；
//   - 「源码注册了、运行期不可见」→ 死绑定（docs/37 记过 `input.action` 注册两次、旧的那份成为
//     死代码这类坑），门直接判红。
//
// 用法：
//   node scripts/gen-lua-api-docs.js                 # 生成 / 增量更新文档桩
//   node scripts/gen-lua-api-docs.js --check        # 门：漂移、桩数超标、死绑定 → 退出码 1
//   node scripts/gen-lua-api-docs.js --update-baseline
//                                                    # 把「待补桩」上限收紧到当前值
//
// 保留规则：人工在 `## \`模块.函数(…)\`` 小节里写下的说明会被保留（识别方式：正文不再含
// `TODO(文档桩)`）；模块标题与第一个小节之间的散文是「模块说明」，同样原样保留。
// 再生成只做三件事：补齐缺失的小节、按基线删掉已不存在的小节、刷新标题里的签名。

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const REPO_ROOT = path.resolve(import.meta.dirname, '..');
const SURFACE_JSON = path.join(
  REPO_ROOT,
  'engine/crates/engine/src/core/api_surface_snapshot/lua_api_surface.json',
);
const API_DIR = path.join(REPO_ROOT, 'engine/crates/engine/src/core/script/api');
const DOC_RS = path.join(REPO_ROOT, 'engine/crates/engine/src/lua_api.rs');
const DOC_DIR = path.join(REPO_ROOT, 'engine/crates/engine/src/lua_api/doc');
const BASELINE_JSON = path.join(DOC_DIR, 'baseline.json');
const STUB_MARKER = 'TODO(文档桩)';
const HEADER_END = '<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->';

const args = new Set(process.argv.slice(2));
const CHECK = args.has('--check');
const UPDATE_BASELINE = args.has('--update-baseline');

function rel(target) {
  return path.relative(REPO_ROOT, target).split(path.sep).join('/');
}

function readText(file) {
  return fs.readFileSync(file, 'utf8').replace(/\r\n/g, '\n');
}

function fail(messages) {
  for (const line of messages) console.error(`  - ${line}`);
  process.exit(1);
}

// ------------------------------------------------------------------ 运行期 API 面
const surface = JSON.parse(readText(SURFACE_JSON));
/** @type {Map<string, string[]>} 模块名 → 排序后的函数名列表 */
const modules = new Map();
for (const table of surface.tables ?? []) {
  modules.set(table.name, [...(table.funcs ?? [])].sort());
}

// ------------------------------------------------------------------ 绑定源码解析
//
// 三种注册形状（实测覆盖 `api/*.rs` 全部 267 处 `create_function`）：
//
//     ① 直写   camera.set("set_active", lua.create_function(move |_, id: i64| { … })?)
//              模块名 = 该变量**之后**第一次 `lua.globals().set("<模块>", <变量>)`；
//              `ui` 表由 mod.rs 建好后传给 ui / ui_panel / ui_nav 共享，没有收尾时退回变量名。
//     ② 宏内   macro_rules! bind_events { ($table:expr, …) => { $table.set("events", …) } }
//              一个宏展开进多张表（`physics` 与 `physics3d` 共用一份实现），按 `NAME!(<变量>, …)`
//              调用点分别记账，签名相同。
//     ③ 循环   for (name, mode) in [("is_down", 0), ("just_pressed", 1)] { input.set(name, …) }
//              名字不在 `.set` 上，取自数组字面量每个元组的第一个字符串，逐个展开。
//
// 另外必须容忍字符串与 `lua.create_function` 之间的 `//` 注释行（`cargo fmt` 不搬注释，
// `ui.text_get` / `input.rumble` 都是这种写法）。

/** @type {{module:string, func:string, sig:string, file:string, line:number, shape:string}[]} */
const bindings = [];
/** @type {string[]} 解析器自己发现的可疑之处（报给人，别让它静默降级） */
const notes = [];

const GLOBALS_PATTERN = /lua\.globals\(\)\.set\(\s*"(?<module>[^"]+)"\s*,\s*(?<var>[A-Za-z_]\w*)/g;
// 变量位可能是宏参数 `$table`（②），所以不能用 `\b` 起头——`$` 不是词字符，`\b` 会让
// 匹配从 `table` 中间开始、把宏注册错认成一张叫 `table` 的表。
const BINDING_PATTERN =
  /(?<![\w$])(?<var>\$?[A-Za-z_]\w*)\.set\(\s*(?:"(?<literal>[A-Za-z_]\w*)"|(?<loopvar>[a-z_]\w*))\s*,(?<gap>(?:\s|\/\/[^\n]*|\/\*[\s\S]*?\*\/)+)lua\.create_function(?:_mut)?\(\s*(?:move\s*)?\|(?<params>[^|]*)\|/g;
const LOOP_PATTERN = /for\s*\((?<patterns>[^)]*)\)\s*in\s*\[(?<items>[\s\S]*?)\]/g;
const MACRO_DEF_PATTERN = /macro_rules!\s*(?<name>[A-Za-z_]\w*)\s*\{/g;
const macroInvocationPattern = (name) =>
  new RegExp(String.raw`(?<![\w$])${name}!\(\s*(?<var>\$?[A-Za-z_]\w*)`, 'g');

function splitTopLevel(text, separator = ',') {
  const parts = [];
  let depth = 0;
  let current = '';
  for (const ch of text) {
    if (ch === '(' || ch === '[' || ch === '{' || ch === '<') depth += 1;
    else if (ch === ')' || ch === ']' || ch === '}' || ch === '>') depth -= 1;
    if (ch === separator && depth === 0) {
      parts.push(current.trim());
      current = '';
    } else {
      current += ch;
    }
  }
  if (current.trim().length > 0) parts.push(current.trim());
  return parts;
}

/** 从闭包参数表推断 Lua 侧参数列表：`|lua, (x, y): (f32, f32)|` → `x: f32, y: f32`。 */
function signatureOf(paramText) {
  const params = splitTopLevel(paramText);
  // 第一个参数恒为 `&Lua` 句柄（`lua` / `_`），真正的 Lua 参数是第二个。
  const raw = params.length >= 2 ? params.slice(1).join(',').trim() : '';
  if (raw === '' || raw === '()') return '';
  const annotated = /^\((?<names>[^)]*)\)\s*:\s*\((?<types>.*)\)$/s.exec(raw);
  if (annotated) {
    const names = splitTopLevel(annotated.groups.names);
    const types = splitTopLevel(annotated.groups.types);
    if (names.length === types.length && names.length > 0) {
      return names.map((name, index) => `${name}: ${types[index]}`).join(', ');
    }
    return `(${annotated.groups.names.replace(/\s+/g, ' ')})`;
  }
  const single = /^(?<name>[A-Za-z_]\w*)\s*:\s*(?<type>.+)$/s.exec(raw);
  if (single) return `${single.groups.name}: ${single.groups.type.replace(/\s+/g, ' ')}`;
  if (/^[A-Za-z_]\w*$/.test(raw)) return `${raw}: ?`;
  return `(${raw.replace(/\s+/g, ' ')})`;
}

/** ②③ 需要的辅助索引：globals 收尾、循环头、宏定义与宏调用点。 */
function indexFileShapes(text) {
  const globals = [];
  for (const match of text.matchAll(GLOBALS_PATTERN)) {
    globals.push({ module: match.groups.module, var: match.groups.var, offset: match.index ?? 0 });
  }
  const loops = [];
  for (const match of text.matchAll(LOOP_PATTERN)) {
    const bound = splitTopLevel(match.groups.patterns ?? '')[0] ?? '';
    const items = match.groups.items ?? '';
    let names = [...items.matchAll(/\(\s*"(?<name>[A-Za-z_]\w*)"/g)].map((entry) => entry.groups.name);
    if (names.length === 0) {
      names = [...items.matchAll(/"(?<name>[A-Za-z_]\w*)"/g)].map((entry) => entry.groups.name);
    }
    loops.push({
      bound,
      names,
      start: match.index ?? 0,
      end: (match.index ?? 0) + match[0].length,
    });
  }
  const macroDefs = [...text.matchAll(MACRO_DEF_PATTERN)].map((match) => ({
    name: match.groups.name,
    offset: match.index ?? 0,
  }));
  const macroCalls = new Map();
  for (const def of macroDefs) {
    // 记下**调用点**的变量与位置：宏体里的偏移没法区分它展开进了哪张表
    // （`bind_events!(physics, …)` 与 `bind_events!(physics3d, …)` 共用同一段源码），
    // 归属必须按调用点之后的 `globals().set("<模块>", <变量>)` 判断。
    const calls = [...text.matchAll(macroInvocationPattern(def.name))].map((match) => ({
      var: match.groups.var,
      offset: match.index ?? 0,
    }));
    const unique = new Map();
    for (const call of calls) {
      if (!unique.has(call.var)) unique.set(call.var, call);
    }
    macroCalls.set(def.name, [...unique.values()]);
  }
  return { globals, loops, macroDefs, macroCalls };
}

/** ①：变量 → 模块名（该变量之后的第一次 `globals().set("<模块>", <变量>)`）。 */
function moduleOfVar(index, varName, offset) {
  for (const candidate of index.globals) {
    if (candidate.var === varName && candidate.offset >= offset) return candidate.module;
  }
  // `ui` 表由 api/mod.rs 建好后传进三个文件共享：本文件里没有收尾的 globals().set。
  return varName;
}

function parseBindingFile(file) {
  const text = readText(file);
  const relFile = rel(file);
  const index = indexFileShapes(text);
  let sites = 0;
  for (const match of text.matchAll(BINDING_PATTERN)) {
    const offset = match.index ?? 0;
    const line = text.slice(0, offset).split('\n').length;
    const sig = signatureOf(match.groups.params);
    sites += 1;

    // 目标函数名：① 字面量；③ 循环变量 → 展开成数组里的每个名字。
    let names = [match.groups.literal];
    let shape = 'direct';
    if (match.groups.literal === undefined) {
      // 取**最近的前置**循环：同一个文件里可以有好几个 `for (name, …) in [...]`
      // （`input.is_down` 系与 `input.mouse_is_down` 系就是两处），按起点最近者归属。
      const loop = index.loops
        .filter((entry) => entry.bound === match.groups.loopvar && entry.start < offset)
        .sort((a, b) => b.start - a.start)[0];
      if (loop === undefined || loop.names.length === 0) {
        notes.push(
          `${relFile}:${line} .set(${match.groups.loopvar}, …) 的名字来源没解析出来（新的注册形状？）`,
        );
        continue;
      }
      names = loop.names;
      shape = 'loop';
    }

    // 目标表：② 宏参数 `$table` 按宏调用点展开成多张表。
    let targets = [{ var: match.groups.var, offset }];
    if (match.groups.var.startsWith('$')) {
      shape = 'macro';
      const enclosing = [...index.macroDefs].reverse().find((def) => def.offset < offset);
      targets = (enclosing === undefined ? undefined : index.macroCalls.get(enclosing.name)) ?? [];
      if (targets.length === 0) {
        notes.push(
          `${relFile}:${line} 宏 ${enclosing?.name ?? '?'} 的调用点没找到，${names.join('/')} 的归属表未定`,
        );
        continue;
      }
    }

    for (const target of targets) {
      const module = moduleOfVar(index, target.var, target.offset);
      for (const name of names) {
        bindings.push({ module, func: name, sig, file: relFile, line, shape });
      }
    }
  }
  // `create_function` 出现次数 ≠ 识别到的绑定点 ⇒ 有解析器没覆盖的注册形状。
  const rawCalls = (text.match(/lua\.create_function/g) ?? []).length;
  if (rawCalls !== sites) {
    notes.push(
      `${relFile} create_function ${rawCalls} 处、只解析到 ${sites} 个绑定点（注册形状变了，签名推断有遗漏）`,
    );
  }
}

for (const entry of fs.readdirSync(API_DIR, { withFileTypes: true })) {
  if (entry.isFile() && entry.name.endsWith('.rs')) {
    parseBindingFile(path.join(API_DIR, entry.name));
  }
}

// 同一个 `模块.函数` 注册两次：后者静默覆盖前者，画面与样例都可能照跑。
const byKey = new Map();
const duplicated = [];
for (const binding of bindings) {
  const key = `${binding.module}.${binding.func}`;
  const previous = byKey.get(key);
  if (previous === undefined) byKey.set(key, binding);
  else {
    duplicated.push(
      `${key}（${previous.file}:${previous.line} 与 ${binding.file}:${binding.line}）`,
    );
  }
}

// ------------------------------------------------------------------ 现有文档桩（保留人工说明）
/**
 * @param {string} module
 * @returns {{preamble: string, sections: Map<string, string>}}
 */
function readExistingDoc(module) {
  const file = path.join(DOC_DIR, `${module}.md`);
  const result = { preamble: '', sections: new Map() };
  if (!fs.existsSync(file)) return result;
  const text = readText(file);
  const headerEnd = text.indexOf(HEADER_END);
  const rest = headerEnd >= 0 ? text.slice(headerEnd + HEADER_END.length) : text;
  const firstSection = rest.search(/^## /m);
  result.preamble = (firstSection >= 0 ? rest.slice(0, firstSection) : rest).trim();
  if (firstSection < 0) return result;
  for (const chunk of rest.slice(firstSection).split(/^## /m).slice(1)) {
    const heading = chunk.split('\n', 1)[0].trim();
    const name = /^`[^`.]+\.([^`(]+)\(/.exec(heading)?.[1];
    if (name !== undefined) result.sections.set(name, `## ${chunk.replace(/\n+$/, '')}\n`);
  }
  return result;
}

// ------------------------------------------------------------------ 生成
const RUST_KEYWORDS = new Set([
  'as', 'break', 'const', 'continue', 'crate', 'dyn', 'else', 'enum', 'extern', 'false', 'fn',
  'for', 'if', 'impl', 'in', 'let', 'loop', 'match', 'mod', 'move', 'mut', 'pub', 'ref', 'return',
  'self', 'static', 'struct', 'super', 'trait', 'true', 'type', 'unsafe', 'use', 'where', 'while',
  'async', 'await', 'box', 'become', 'do', 'final', 'macro', 'override', 'priv', 'try', 'typeof',
  'unsized', 'virtual', 'yield',
]);

const SHAPE_TEXT = { direct: '直写', macro: '宏展开', loop: '循环注册' };

function sectionHeading(module, func, sig) {
  return `## \`${module}.${func}(${sig === undefined ? '…' : sig})\``;
}

function stubBody(module, func, binding) {
  const source = binding
    ? `绑定：\`${binding.file}:${binding.line}\`（${SHAPE_TEXT[binding.shape]}）`
    : '绑定：未定位（注册形状不在解析器覆盖范围内，签名待人工补）';
  return [
    sectionHeading(module, func, binding?.sig),
    '',
    `> ${source}；签名由 \`scripts/gen-lua-api-docs.js\` 从绑定源码推断，需人工核对。`,
    '',
    `${STUB_MARKER}：一句话说明这个函数做什么、什么时候该用它。`,
    '',
    '- 参数：',
    '- 返回：',
    '- 失败：',
    '- 示例：',
    '',
  ].join('\n');
}

const drift = [];
let stubs = 0;
let funcs = 0;
const generatedModules = [];

fs.mkdirSync(DOC_DIR, { recursive: true });

for (const [module, list] of [...modules.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
  const existing = readExistingDoc(module);
  const out = [
    `# Lua 模块 \`${module}\``,
    '',
    `> 运行期 API 面基线里本模块有 ${list.length} 个函数。本页由 \`scripts/gen-lua-api-docs.js\` 生成：`,
    '> **函数清单与标题里的签名**不要手改；在每个小节里把',
    `> \`${STUB_MARKER}\` 换成真实说明即可，再生成会保留已补的内容。`,
    '',
    HEADER_END,
    '',
  ];
  if (existing.preamble !== '') out.push(existing.preamble, '');
  for (const func of list) {
    funcs += 1;
    const binding = byKey.get(`${module}.${func}`);
    const previous = existing.sections.get(func);
    const keep = previous !== undefined && !previous.includes(STUB_MARKER);
    // 人工说明保留，但标题里的签名跟着源码走（绑定参数变了，文档桩立刻反映）。
    const section = keep
      ? `${sectionHeading(module, func, binding?.sig)}\n${previous.split('\n').slice(1).join('\n')}`
      : stubBody(module, func, binding);
    if (!keep) stubs += 1;
    out.push(section.trimEnd(), '');
  }
  const expected = `${out.join('\n').replace(/\n+$/, '')}\n`;
  const file = path.join(DOC_DIR, `${module}.md`);
  const current = fs.existsSync(file) ? readText(file) : null;
  if (current !== expected) {
    if (CHECK) drift.push(`${rel(file)} 与再生成结果不一致（函数清单或签名变了：跑生成器并提交）`);
    else fs.writeFileSync(file, expected);
  }
  generatedModules.push({ module, count: list.length });
}

// 基线里有、源码定位不到 ⇒ 解析器覆盖不足（打印出来，别当成"没有签名"就行）。
const unmatched = [];
for (const [module, list] of modules.entries()) {
  for (const func of list) {
    if (!byKey.has(`${module}.${func}`)) unmatched.push(`${module}.${func}`);
  }
}
// 源码注册了、运行期基线里没有 ⇒ 死绑定。
const dead = [...byKey.keys()].filter((key) => {
  const dot = key.indexOf('.');
  const list = modules.get(key.slice(0, dot));
  return list === undefined || !list.includes(key.slice(dot + 1));
});

const rsOut = [
  '//! Lua API 参考（**本文件与 `src/lua_api/doc/*.md` 由脚本生成，勿手改**）。',
  '//!',
  '//! - 事实源：运行期 `_G` 探针导出的 API 面基线（`core::api_surface_snapshot` 的快照门，',
  `//!   \`lua_api_surface.json\`，${generatedModules.length} 个模块表 / ${funcs} 个函数）；`,
  '//! - 再生成：`node scripts/gen-lua-api-docs.js`；说明文字写在各模块的 markdown 里',
  '//!   （`src/lua_api/doc/<模块>.md`），Lua 侧签名从绑定源码推断；',
  '//! - 门：`node scripts/gen-lua-api-docs.js --check`——再生成必须是 no-op、待补桩数不得',
  '//!   超标、不许出现死绑定与重复注册；',
  `//! - 还没补的函数在小节里标着 \`${STUB_MARKER}\`（计数见 \`doc/baseline.json\`）。`,
  '//!',
  '//! 这些模块只带文档、不带任何条目：Lua 侧的名字才是脚本契约，Rust 侧不放同名函数，',
  '//! 免得两边各活一份、互相说不清谁是真的。',
  '',
];
for (const { module } of generatedModules) {
  const ident = RUST_KEYWORDS.has(module) ? `r#${module}` : module;
  rsOut.push(`pub mod ${ident} {`);
  rsOut.push(`    #![doc = include_str!("lua_api/doc/${module}.md")]`);
  rsOut.push('}');
  rsOut.push('');
}
const rsExpected = `${rsOut.join('\n').replace(/\n+$/, '')}\n`;
const rsCurrent = fs.existsSync(DOC_RS) ? readText(DOC_RS) : null;
if (rsCurrent !== rsExpected) {
  if (CHECK) drift.push(`${rel(DOC_RS)} 与再生成结果不一致（模块清单或函数数量变了）`);
  else fs.writeFileSync(DOC_RS, rsExpected);
}

// ------------------------------------------------------------------ 桩数基线
const baseline = fs.existsSync(BASELINE_JSON)
  ? JSON.parse(readText(BASELINE_JSON))
  : { funcs: 0, stubs: Number.MAX_SAFE_INTEGER, documented: 0 };
const payload = `${JSON.stringify({ funcs, stubs, documented: funcs - stubs }, null, 2)}\n`;
const currentPayload = fs.existsSync(BASELINE_JSON) ? readText(BASELINE_JSON) : null;
if (UPDATE_BASELINE) {
  if (currentPayload !== payload) fs.writeFileSync(BASELINE_JSON, payload);
} else if (CHECK) {
  if (stubs > baseline.stubs) {
    drift.push(
      `待补文档桩 ${stubs} > 基线 ${baseline.stubs}：补文档应当让桩数下降；确属新增 API 时跑 --update-baseline 并说明理由`,
    );
  } else if (stubs < baseline.stubs) {
    drift.push(
      `待补文档桩已降到 ${stubs}（基线 ${baseline.stubs}）：这是好事，跑 --update-baseline 收紧基线再提交`,
    );
  }
} else if (currentPayload !== payload) {
  fs.writeFileSync(BASELINE_JSON, payload);
}

// ------------------------------------------------------------------ 汇报
const mode = CHECK ? 'check' : UPDATE_BASELINE ? 'update-baseline' : 'write';
console.log(
  `lua_api_docs mode=${mode} tables=${modules.size} funcs=${funcs} stubs=${stubs} documented=${funcs - stubs}`,
);
if (unmatched.length > 0) {
  console.log(
    `unmatched=${unmatched.length}（基线有、源码未定位到绑定点）: ${unmatched.slice(0, 12).join(', ')}${unmatched.length > 12 ? ', …' : ''}`,
  );
}
if (dead.length > 0) console.log(`dead_bindings=${dead.length}: ${dead.join(', ')}`);
for (const note of notes) console.log(`note: ${note}`);

if (CHECK) {
  const problems = [
    ...drift,
    ...dead.map((key) => `死绑定 ${key}：注册了但运行期 API 面里没有`),
    ...duplicated.map((text) => `重复注册 ${text}：后者静默覆盖前者`),
  ];
  if (problems.length > 0) {
    console.error('[RED] lua_api_docs_check_failed');
    fail(problems);
  }
  console.log('lua_api_docs_ok 再生成是 no-op、桩数未超标、无死绑定/重复注册');
}
