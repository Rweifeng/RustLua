#!/usr/bin/env node
// CI 少跑测试的账本门（M78-00 §1.3「flaky skip 有账本」）。
//
// 任何读取 `RUSTLUA_SKIP_CI_FLAKY` 的测试都会让 CI 的通过数失真：跳掉的那条用例
// 在日志里只是一行黄色文字，没人知道少跑了什么、什么时候能还。本门把「谁在跳、
// 跳哪条、为什么、什么条件恢复、影响面」变成 `scripts/verify/flaky-skips.json`
// 里的强制登记：新增跳过不登记 → 红；登记过期（reader 里已没有这个 env / 用例名）→ 红；
// 字段缺失 → 红。
//
// 口径与其余 node 门一致：判红即非零退出；`--self-test` 用内存 fixture 证明它会红。
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';

const ENV_VAR = 'RUSTLUA_SKIP_CI_FLAKY';
const REQUIRED_FIELDS = [
  'test',
  'reader',
  'issuer',
  'since',
  'reason',
  'restore_condition',
  'impact',
  'review_by',
];
const SCAN_ROOTS = ['engine', 'scripts', 'plugins', 'examples', path.join('.github', 'workflows')];
const SCAN_EXT = new Set(['.rs', '.ps1', '.mjs', '.js', '.yml']);

function walk(absRoot, out) {
  if (!fs.existsSync(absRoot)) return out;
  for (const entry of fs.readdirSync(absRoot, { withFileTypes: true })) {
    if (entry.name === 'target' || entry.name === '.git' || entry.name === 'node_modules') continue;
    const full = path.join(absRoot, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else if (SCAN_EXT.has(path.extname(entry.name))) out.push(full);
  }
  return out;
}

// 从跳过打印里取用例名：`skip <name> (RUSTLUA_SKIP_CI_FLAKY)`。
function skippedTestsIn(text) {
  const names = new Set();
  for (const at of allIndices(text, ENV_VAR)) {
    const window = text.slice(Math.max(0, at - 600), at + 600);
    for (const match of window.matchAll(/skip\s+([A-Za-z_][A-Za-z0-9_]*)/g)) names.add(match[1]);
  }
  return [...names].sort();
}

function allIndices(text, needle) {
  const hits = [];
  for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + needle.length)) {
    hits.push(at);
  }
  return hits;
}

export function audit({ repoRoot, ledger }) {
  const errors = [];
  const relSkip = new Map();
  for (const root of SCAN_ROOTS) {
    for (const file of walk(path.join(repoRoot, root), [])) {
      const text = fs.readFileSync(file, 'utf8');
      if (!text.includes(ENV_VAR)) continue;
      const rel = path.relative(repoRoot, file).replaceAll('\\', '/');
      const names = skippedTestsIn(text);
      if (names.length === 0 && rel.endsWith('.rs')) {
        // 只有测试源文件才是"真跳过点"；脚本/文档里提到这个 env 是在描述门，不是少跑用例。
        errors.push(`${rel} 读取了 ${ENV_VAR} 但没打印被跳过的用例名（CI 少跑什么无从得知）`);
      }
      for (const name of names) relSkip.set(name, rel);
    }
  }
  if (!Array.isArray(ledger.skips)) {
    return { errors: [...errors, '账本缺少 skips 数组'], stats: { readers: relSkip.size } };
  }
  const ledgerTests = new Set();
  for (const [index, entry] of ledger.skips.entries()) {
    const where = `账本第 ${index + 1} 条（${entry?.test ?? '无名'}）`;
    for (const field of REQUIRED_FIELDS) {
      const value = entry?.[field];
      if (typeof value !== 'string' || value.trim() === '') {
        errors.push(`${where} 缺字段 ${field}（原因/恢复条件/影响范围都必须写清）`);
      }
    }
    if (!entry?.test) continue;
    ledgerTests.add(entry.test);
    const reader = entry.reader ? path.join(repoRoot, entry.reader) : '';
    if (!reader || !fs.existsSync(reader)) {
      errors.push(`${where} 的 reader 不存在：${entry.reader}`);
      continue;
    }
    const readerText = fs.readFileSync(reader, 'utf8');
    if (!readerText.includes(ENV_VAR) || !readerText.includes(entry.test)) {
      errors.push(
        `${where} 登记已过期：${entry.reader} 里既找不到 ${ENV_VAR} 也找不到用例名 —— 请把这条删掉（用例已回到 CI）`,
      );
    }
    const issuer = entry.issuer ? path.join(repoRoot, entry.issuer) : '';
    if (!issuer || !fs.existsSync(issuer) || !fs.readFileSync(issuer, 'utf8').includes(ENV_VAR)) {
      errors.push(`${where} 的 issuer（设置 ${ENV_VAR} 的地方）不存在或没设置该变量：${entry.issuer}`);
    }
  }
  for (const [name, rel] of relSkip) {
    if (!ledgerTests.has(name)) {
      errors.push(`未登记的跳过：${rel} 在 ${ENV_VAR} 下跳过 ${name}（必须先写进账本，含原因/恢复条件/影响范围）`);
    }
  }
  if (relSkip.size === 0 && ledger.skips.length > 0) {
    errors.push('源码里已没有任何用例读取 env，但账本仍有 ' + ledger.skips.length + ' 条 —— 全部删除');
  }
  return { errors, stats: { registered: ledger.skips.length, skipSites: relSkip.size } };
}

function selfTest() {
  const cases = [];
  const base = {
    test: 'a_works',
    reader: 'engine/x.rs',
    issuer: '.github/workflows/quality.yml',
    since: '2026-01-01',
    reason: 'CI 崩溃',
    restore_condition: '连续 3 次绿',
    impact: '少跑一条',
    review_by: '2026-12-31',
  };
  const reader = `fn a_works() {\n  if std::env::var_os("${ENV_VAR}").is_some() {\n    println!("skip a_works (${ENV_VAR})");\n    return;\n  }\n}\n`;
  const tree = {
    'engine/x.rs': reader,
    '.github/workflows/quality.yml': `env:\n  ${ENV_VAR}: "1"\n`,
  };
  cases.push(['完整登记为绿', tree, { skips: [base] }, false]);
  cases.push(
    ['未登记的跳过年红', { ...tree, 'engine/y.rs': `println!("skip b_works (${ENV_VAR})"); if std::env::var_os("${ENV_VAR}").is_some() {}` }, { skips: [base] }, true],
  );
  cases.push(['缺字段红', { ...tree }, { skips: [{ ...base, impact: '' }] }, true]);
  cases.push(['过期登记红', { ...tree, 'engine/x.rs': 'fn a_works() {}' }, { skips: [base] }, true]);
  let failed = 0;
  for (const [name, files, ledger, wantRed] of cases) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-skip-'));
    for (const [rel, text] of Object.entries(files)) {
      const abs = path.join(dir, rel);
      fs.mkdirSync(path.dirname(abs), { recursive: true });
      fs.writeFileSync(abs, text);
    }
    const { errors } = audit({ repoRoot: dir, ledger });
    const gotRed = errors.length > 0;
    if (gotRed !== wantRed) {
      failed += 1;
      console.error(`自测失败：${name} 期望 ${wantRed ? '红' : '绿'}，实际 ${gotRed ? '红' : '绿'} ${errors}`);
    }
    fs.rmSync(dir, { recursive: true, force: true });
  }
  if (failed > 0) process.exit(1);
  console.log(`flaky_skip_gate_selftest_ok cases=${cases.length}`);
}

if (process.argv.includes('--self-test')) {
  selfTest();
} else {
  const repoRoot = path.resolve(import.meta.dirname, '..', '..');
  const ledgerPath = path.join(repoRoot, 'scripts', 'verify', 'flaky-skips.json');
  const ledger = JSON.parse(fs.readFileSync(ledgerPath, 'utf8'));
  if (ledger.env_var !== ENV_VAR) {
    console.error(`账本 env_var=${ledger.env_var}，与门期望的 ${ENV_VAR} 不一致`);
    process.exit(1);
  }
  const { errors, stats } = audit({ repoRoot, ledger });
  for (const error of errors) console.error('flaky_skip: ' + error);
  if (errors.length > 0) process.exit(1);
  console.log(
    `flaky_skip_gate_ok registered=${stats.registered} skip_sites=${stats.skipSites} ` +
      `（CI 在 ${ENV_VAR}=1 下少跑 ${stats.registered} 条用例，逐条原因见 scripts/verify/flaky-skips.json）`,
  );
}
