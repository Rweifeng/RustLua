#!/usr/bin/env node
// Web 浏览器运行时证据的受限登记门（M83-04 §三-1）。
//
// 为什么需要它：`verify.ps1 -Web` 在无可用 WebGL2 的环境里以 exit 3 (BLOCKED) 结束，
// CI 步骤只发一条 warning、不判红。这解决了"环境缺口冒充代码回归"，但留下另一个病：
// **BLOCKED 可以永远绿下去**——等于"浏览器运行时证据"名义存在、实际从未取得。
// 本门把现状登记在 `metrics/web-evidence.json`，并给它一个到期日：到期仍 blocked ⇒ 判 2。
//
// 用法：
//   node scripts/verify/check-web-evidence-policy.mjs            # 到期裁定（默认 --check）
//   node scripts/verify/check-web-evidence-policy.mjs --check    # 同上
//   node scripts/verify/check-web-evidence-policy.mjs --list     # 只打印状态
//   node scripts/verify/check-web-evidence-policy.mjs --self-test # 反面对照（证明门会红）
//
// 退出码：0 = 通过（ok，或 blocked 但未到期）；2 = 判红（blocked 且已到期 / 登记缺失或格式坏 / 自测失败）。
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, '..', '..');
const REGISTRY_REL = 'metrics/web-evidence.json';

/** 读登记并做结构校验；返回 { status, blocked } 或抛错。 */
export function readRegistry(repoRoot) {
  const file = path.join(repoRoot, REGISTRY_REL);
  if (!fs.existsSync(file)) throw new Error(`登记文件缺失：${REGISTRY_REL}`);
  const data = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (data?.schema !== 1) throw new Error(`${REGISTRY_REL} 的 schema 必须是 1`);
  const status = data?.status;
  if (status !== 'ok' && status !== 'blocked') {
    throw new Error(`${REGISTRY_REL} 的 status 只能是 ok 或 blocked（实际 ${JSON.stringify(status)}）`);
  }
  if (status === 'blocked') {
    const blocked = data?.blocked;
    if (!blocked || typeof blocked !== 'object') throw new Error(`status=blocked 时必须有 blocked 段`);
    for (const key of ['reason', 'since', 'review_by']) {
      if (!blocked[key] || typeof blocked[key] !== 'string') {
        throw new Error(`blocked.${key} 缺失或不是字符串`);
      }
    }
    if (!/^\d{4}-\d{2}-\d{2}$/.test(blocked.review_by)) {
      throw new Error(`blocked.review_by 必须是 YYYY-MM-DD（实际 ${blocked.review_by}）`);
    }
  }
  return { status, blocked: data?.blocked };
}

/** 到期判定：blocked 且 today > review_by ⇒ 判红。返回 errors 数组。 */
export function audit(registry, today) {
  const errors = [];
  if (registry.status !== 'blocked') return errors;
  const reviewBy = new Date(`${registry.blocked.review_by}T23:59:59Z`);
  if (Number.isNaN(reviewBy.getTime())) {
    errors.push(`review_by 不是合法日期：${registry.blocked.review_by}`);
    return errors;
  }
  if (today.getTime() > reviewBy.getTime()) {
    errors.push(
      `Web 浏览器运行时证据仍 BLOCKED，且已过评审日 review_by=${registry.blocked.review_by}` +
        `（since=${registry.blocked.since}）：${registry.blocked.reason}。` +
        '处理方式见登记文件的 unblock_conditions；不允许继续静默受限。',
    );
  }
  return errors;
}

function printStatus(registry) {
  if (registry.status === 'ok') {
    console.log('web_evidence_policy status=ok（浏览器运行时证据为正题，无受限登记）');
    return;
  }
  const b = registry.blocked;
  console.log(
    `web_evidence_policy status=blocked since=${b.since} review_by=${b.review_by} reason=${b.reason}`,
  );
}

function selfTest() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'web-evidence-policy-'));
  const cases = [];
  const write = (name, payload) => {
    const root = path.join(dir, name);
    fs.mkdirSync(path.join(root, 'metrics'), { recursive: true });
    fs.writeFileSync(path.join(root, REGISTRY_REL), JSON.stringify(payload), 'utf8');
    return root;
  };
  const future = new Date(Date.now() + 24 * 3600 * 1000);
  const past = new Date(Date.now() - 24 * 3600 * 1000);
  const iso = (d) => d.toISOString().slice(0, 10);

  // 1 绿：blocked 且未到期
  {
    const root = write('green-future', {
      schema: 1,
      status: 'blocked',
      blocked: { reason: 'x', since: '2026-01-01', review_by: iso(future) },
    });
    const reg = readRegistry(root);
    const errs = audit(reg, new Date());
    cases.push({ name: 'blocked_not_expired_is_green', ok: errs.length === 0, errors: errs });
  }
  // 2 红：blocked 且已到期
  {
    const root = write('red-expired', {
      schema: 1,
      status: 'blocked',
      blocked: { reason: 'x', since: '2026-01-01', review_by: iso(past) },
    });
    const reg = readRegistry(root);
    const errs = audit(reg, new Date());
    cases.push({ name: 'blocked_expired_is_red', ok: errs.length === 1, errors: errs });
  }
  // 3 绿：status=ok
  {
    const root = write('green-ok', { schema: 1, status: 'ok' });
    const reg = readRegistry(root);
    const errs = audit(reg, new Date());
    cases.push({ name: 'ok_is_green', ok: errs.length === 0, errors: errs });
  }
  // 4 红：格式坏（缺 review_by）
  {
    const root = write('red-malformed', {
      schema: 1,
      status: 'blocked',
      blocked: { reason: 'x', since: '2026-01-01' },
    });
    let threw = false;
    try {
      readRegistry(root);
    } catch {
      threw = true;
    }
    cases.push({ name: 'malformed_is_red', ok: threw, errors: [] });
  }

  fs.rmSync(dir, { recursive: true, force: true });
  const failed = cases.filter((c) => !c.ok);
  for (const c of cases) {
    console.log(`  ${c.ok ? 'ok  ' : 'FAIL'} ${c.name}`);
    for (const e of c.errors) console.log(`       ${e}`);
  }
  if (failed.length > 0) {
    console.error(`web_evidence_policy_selftest_failed cases=${cases.length} failed=${failed.length}`);
    return 2;
  }
  console.log(`web_evidence_policy_selftest_ok cases=${cases.length}（1 绿 + 3 红）`);
  return 0;
}

function main(argv) {
  const args = new Set(argv.slice(2));
  if (args.has('--self-test')) return selfTest();

  let registry;
  try {
    registry = readRegistry(REPO_ROOT);
  } catch (error) {
    console.error(`[RED] ${error.message}`);
    return 2;
  }
  printStatus(registry);
  if (args.has('--list')) return 0;

  const errors = audit(registry, new Date());
  for (const error of errors) console.error(`[RED] ${error}`);
  if (errors.length > 0) return 2;
  console.log('web_evidence_policy_ok');
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv);
}
