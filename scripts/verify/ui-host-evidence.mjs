// Web / 微信的 UI 证据驱动（scripts/verify/ui-host-evidence.ps1 调用）。
//
// 分工：wasm harness（engine/crates/rustlua-ui-host-harness）在 **wasm32 宿主**上执行
// "`.ui.json` → 场景 → RuntimeScene → 版面/顶点读数"，判定逻辑与 pc 例子共用
// `rustlua-ui-host-evidence`；本脚本只负责两件宿主相关的事：
//   1. 用宿主自己的胶水形态装载 wasm —— web 用 `--target web` 的 ESM，
//      微信按仓库既有约定用 `--target no-modules` + `module.exports` 补丁
//      （与 scripts/lib/wasm-lua/verify-no-modules-bridge.mjs 同一套，见 package.ps1 的微信补丁）；
//   2. 把绿读数落成产物，并（可选）写回 `ui/hosts.json` 的对应宿主条目。
//
// 这套 harness **不加载**浏览器 canvas/WebGL，也不装字体图集：它测的是文档导出与引擎
// UI 解算/提交这一段，与 `pc` 同口径。产物里 `sdf_vertices=0` 就是这条边界的读数。
// 微信真机 WXWebAssembly 的行为不在本机复刻范围（docs/52 R1），这里只证明 no-modules
// 胶水装载下的同一份 wasm 读数；产物 note 必须把这句话带上，不能让"Node 跑通"看起来
// 像"真机跑通"。
//
// 用法：
//   node scripts/verify/ui-host-evidence.mjs --host web|wechat [--document ui/matrix.ui.json]
//        [--project examples/game_api] [--glue-dir artifacts/ui-host-harness/<host>]
//        [--output examples/game_api/artifacts/ui-matrix/<host>.json]
//        [--command "<可复现命令行>"] [--note "<清单备注>"] [--write-hosts]
//
// 退出码：0 = 绿并已落盘；1 = 红/断言不成立；3 = 环境缺失（胶水或文档找不到、node 版本不够）。
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');

const args = parseArgs(process.argv.slice(2));
const document = args.document;
const project = resolve(repoRoot, args.project);
const documentPath = join(project, document);
const glueDir = resolve(repoRoot, args.glueDir);
const outputPath = resolve(repoRoot, args.output);
// 清单是**工程内**文件（`.ui.json` 的资产来源按工程根解析），所以 `--hosts-file` 也按工程根解析。
const hostsPath = resolve(project, args.hostsFile);

if (!['web', 'wechat'].includes(args.host)) {
  fail(3, `--host 只认 web|wechat，实际 ${JSON.stringify(args.host)}`);
}
if (!existsSync(documentPath)) {
  fail(3, `文档不存在：${documentPath}`);
}
const projectTomlPath = join(project, 'project.toml');
if (!existsSync(projectTomlPath)) {
  fail(3, `工程配置不存在：${projectTomlPath}（宿主靠它读 text_render_mode 等，缺了就不是同一套配置）`);
}
if (!existsSync(join(glueDir, 'ui_host_harness.js'))) {
  fail(3, `胶水不存在：${glueDir}（先跑 scripts/verify/ui-host-evidence.ps1 构建，或用 -SkipBuild 复用）`);
}

const specText = readFileSync(documentPath, 'utf8');
const projectToml = readFileSync(projectTomlPath, 'utf8');
const glue = await loadGlue(args.host, glueDir);
const raw = glue.ui_host_evidence(document, args.host, args.command, specText, projectToml);
const result = JSON.parse(raw);

if (result.error) {
  fail(1, `harness 失败：${result.error}`);
}
if (!result.ok || !Array.isArray(result.problems) || result.problems.length > 0) {
  console.error(`ui_host_evidence RED host=${args.host} document=${document} revision=${result.revision ?? '?'}`);
  for (const problem of result.problems ?? []) console.error(`  ✗ ${problem}`);
  fail(1, '读数判红，不写产物（证据只能来自绿的一次运行）');
}

const report = result.report;
const checks = report.checks ?? [];
for (const expected of ['layout_rects', 'draw_order']) {
  if (!checks.includes(expected)) fail(1, `产物缺少检查项 ${expected}`);
}
if (report.document !== document || report.host !== args.host) {
  fail(1, `产物自述 document=${report.document} host=${report.host} 与本次不一致`);
}
if (Number(report.revision) !== Number(result.revision)) {
  fail(1, `产物 revision=${report.revision} 与 harness 现算 ${result.revision} 不一致`);
}

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
// `--note` 显式给了就用它；否则按**本次读数**生成（把原始计数与文本回退摊开，避免
// "检查项一致"被读成"连原始顶点数都一样"）。
const note = args.note || defaultNote(args.host, report);
console.log(
  `ui_host_evidence host=${args.host} document=${document} revision=${report.revision} ` +
    `entities=${report.entities} layout_compared=${report.layout_rects.compared} ` +
    `layout_unreconciled=${report.layout_rects.unreconciled_rows_count} ` +
    `draw_blocks=${report.draw_order.blocks} placement_ok=${report.draw_order.monotonic_preorder} ` +
    `artifact=${args.output}`,
);

if (args.writeHosts) {
  updateHosts(note);
  console.log(`hosts.json updated: ${args.hostsFile}#${document}/${args.host} (${hostsPath})`);
}
process.exitCode = 0;

async function loadGlue(host, directory) {
  if (host === 'web') {
    // `--target web`：ESM 导出 initSync + 各 wasm 导出函数。Node 24 原生支持 ESM 导入。
    const module = await import(pathToFileURL(join(directory, 'ui_host_harness.js')).href);
    module.initSync({ module: readFileSync(join(directory, 'ui_host_harness_bg.wasm')) });
    return module;
  }
  // `--target no-modules`：顶层的 `let wasm_bindgen` 不是 CommonJS 导出，微信代码包靠
  // package.ps1 在胶水末尾补 `module.exports = wasm_bindgen`（本仓库的既有补丁）。
  const require = createRequire(import.meta.url);
  const source = readFileSync(join(directory, 'ui_host_harness.js'), 'utf8');
  const patched = join(directory, 'ui_host_harness.patched.js');
  writeFileSync(patched, `${source}\nmodule.exports = wasm_bindgen;\n`, 'utf8');
  const glue = require(patched);
  glue.initSync({ module: readFileSync(join(directory, 'ui_host_harness_bg.wasm')) });
  return glue;
}

function updateHosts(note) {
  let hosts = { version: 1, documents: {} };
  if (existsSync(hostsPath)) {
    hosts = JSON.parse(readFileSync(hostsPath, 'utf8'));
  }
  if (hosts.version !== 1) fail(1, `hosts.json 只支持 version=1，实际 ${hosts.version}`);
  hosts.documents ??= {};
  const documents = hosts.documents;
  const key = Object.keys(documents).find((item) => normalize(item) === normalize(document)) ?? document;
  documents[key] ??= {};
  documents[key][args.host] = {
    command: args.command,
    checks: [...checks],
    // 清单里的 artifact 与 `document` 一样按**工程根**解析（review.rs 的验收口径），
    // 不能写仓库相对路径，否则发布门读不到产物、把已测降级成 untested。
    artifact: normalize(relative(project, outputPath)),
    revision: Number(report.revision),
    note,
  };
  mkdirSync(dirname(hostsPath), { recursive: true });
  writeHostsWithRetry(`${JSON.stringify(hosts, null, 2)}\n`);
}

function writeHostsWithRetry(text) {
  // Windows 上紧接着的第二个宿主进程会撞上杀软/索引器短时持有 hosts.json，
  // `open` 报 UNKNOWN/-4094。一次瞬时锁不该让一份已经算绿的证据判红，重试几次再抛。
  for (let attempt = 1; ; attempt += 1) {
    try {
      writeFileSync(hostsPath, text, 'utf8');
      return;
    } catch (error) {
      if (attempt >= 5) throw error;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 80 * attempt);
    }
  }
}

/// 清单 note 的默认文案：先写"证明到什么"，再写"没证明什么"，最后把本次原始计数摊开。
function defaultNote(host, report) {
  const layout = report.layout_rects;
  const draw = report.draw_order;
  const readings =
    `读数：版面 ${layout.compared} 键（最大偏差 ${layout.max_abs_delta} px）、` +
    `块序 ${draw.blocks} 块（${draw.monotonic_preorder ? '单调' : '非单调'}）；` +
    `原始 vertices=${report.vertices}、segments=${report.segments}、sdf_vertices=${report.sdf_vertices}——` +
    '本 harness 不带工程资源，文本回落到内建图集，原始计数高于 pc；两项检查不含文本。';
  const boundary =
    '不加载浏览器/小游戏 canvas 与 WebGL，也不证明 WXWebAssembly 真机行为（docs/52 R1）。';
  if (host === 'web') {
    return `Web harness：\`--target web\` 胶水在 Node 的 WebAssembly 里执行同一台 RuntimeScene，` +
      `工程配置经 vfs 挂载；${readings}${boundary}`;
  }
  return `微信 harness：同一份 wasm 产物按 \`--target no-modules\` 胶水装载（module.exports 补丁，` +
    `与 wechat-minigame-harness 同款）；${readings}${boundary}`;
}

function normalize(path) {
  return path.replaceAll('\\', '/').replace(/^\.\//, '');
}

function parseArgs(argv) {
  const values = {
    host: '',
    document: 'ui/matrix.ui.json',
    project: 'examples/game_api',
    glueDir: '',
    output: '',
    hostsFile: 'ui/hosts.json',
    command: '',
    note: '',
    writeHosts: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    const take = () => {
      const value = argv[++i];
      if (value === undefined) fail(3, `${flag} 缺少取值`);
      return value;
    };
    switch (flag) {
      case '--host':
        values.host = take();
        break;
      case '--document':
        values.document = take();
        break;
      case '--project':
        values.project = take();
        break;
      case '--glue-dir':
        values.glueDir = take();
        break;
      case '--output':
        values.output = take();
        break;
      case '--hosts-file':
        values.hostsFile = take();
        break;
      case '--command':
        values.command = take();
        break;
      case '--note':
        values.note = take();
        break;
      case '--write-hosts':
        values.writeHosts = true;
        break;
      default:
        fail(3, `未知参数 ${JSON.stringify(flag)}`);
    }
  }
  if (!values.host) fail(3, '必须给 --host web|wechat');
  values.glueDir ||= `artifacts/ui-host-harness/${values.host}`;
  values.output ||= `examples/game_api/artifacts/ui-matrix/${values.host}.json`;
  values.command ||= `powershell -NoProfile -File scripts/verify/ui-host-evidence.ps1 -Target ${values.host}`;
  return { ...values, document: normalize(values.document) };
}

function fail(code, message) {
  console.error(`ui_host_evidence_failed: ${message}`);
  process.exitCode = code;
  // 顶层没有 await 之后的继续点：直接结束，避免继续写产物。
  process.exit(code);
}
