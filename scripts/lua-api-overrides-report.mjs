// WP7-83：把 `plugins/webgl/lua_api_overrides.json`（平台差异/缺口登记表）导出成**可读工单**。
//
// 存在理由：这份表是"web/微信 API 面与原引擎差在哪"的权威来源，但它的嵌套很深
// （groups[] → {module, note, apis[]} → 每条 API 自带 dropped[]/invented[]），
// 用行级文本统计会**错位**（本项目已因此得出过一次"35 个 module / 24 份 dropped"的错误配对）。
// 这里只做一件事：**按结构**遍历，把每条有缺口（dropped 非空）或自造（invented 非空）的 API 打出来。
//
// 用法：node scripts/lua-api-overrides-report.mjs [--json]
//   --json  以 JSON 输出（便于其他工具消费）
// 退出码恒为 0：这是**报告**工具，不承担"平台性差异算不算问题"的判断。
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const registryPath = join(here, '..', 'plugins', 'webgl', 'lua_api_overrides.json');
const registry = JSON.parse(readFileSync(registryPath, 'utf8'));

const rows = [];
function visit(node, module) {
  if (Array.isArray(node)) { for (const v of node) visit(v, module); return; }
  if (!node || typeof node !== 'object') return;
  const mod = typeof node.module === 'string' ? node.module : module;
  const dropped = Array.isArray(node.dropped) ? node.dropped : null;
  const invented = Array.isArray(node.invented) ? node.invented : null;
  if (dropped || invented) {
    rows.push({
      module: mod || '(未标注)',
      api: node.name || node.fn || node.func || node.api || node.key || node.symbol || '(未命名)',
      dropped: dropped || [],
      invented: invented || [],
      line: node.line || null,
    });
  }
  for (const key of Object.keys(node)) {
    if (key === 'dropped' || key === 'invented') continue;
    visit(node[key], mod);
  }
}
// 顶层是**两个**区块，必须都走（曾因只走 groups 得到"条目总数 0"）：
//   * groups[]       = { module, note, apis: [名字字符串] } —— 只记覆盖了哪些 API，没有 dropped/invented；
//   * replacements[] = 逐条替换/缺口登记（module/dropped/invented/line）—— **缺口在这里**。
visit([registry.groups || [], registry.replacements || []], null);

const withDrops = rows.filter((r) => r.dropped.length > 0);
const withInvents = rows.filter((r) => r.invented.length > 0);

if (process.argv.includes('--json')) {
  console.log(JSON.stringify({ total: rows.length, withDrops, withInvents }, null, 2));
} else {
  console.log('登记表: ' + registryPath);
  console.log('条目总数=' + rows.length + '  有缺口=' + withDrops.length + '  有自造名=' + withInvents.length);
  const byModule = new Map();
  for (const r of withDrops) {
    if (!byModule.has(r.module)) byModule.set(r.module, []);
    byModule.get(r.module).push(r);
  }
  for (const [mod, list] of [...byModule.entries()].sort((a, b) => b[1].length - a[1].length)) {
    const names = list.flatMap((r) => r.dropped);
    console.log('  ' + mod.padEnd(18) + ' 缺口 ' + String(names.length).padStart(2)
      + '  API ' + String(list.length).padStart(2) + '  [' + names.slice(0, 6).join(', ')
      + (names.length > 6 ? ', …' : '') + ']');
  }
  if (withInvents.length) {
    console.log('  自造名（web 侧新增、原引擎没有）：');
    for (const r of withInvents) console.log('    ' + r.module + '.' + r.api + ' → ' + r.invented.join(', '));
  }
}