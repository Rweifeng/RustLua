#!/usr/bin/env node
/**
 * 把 RustLua 编辑器注册成 DSH 的 MCP server（stdio）。
 *
 * 只在 DSH profile 的 cordis.patch.yml 里维护一段带标记的块：
 *   # >>> rustlua-mcp >>>
 *   ...
 *   # <<< rustlua-mcp <<<
 * 重复运行会先删旧块再写新块；不会碰文件里的其它配置。改完 DSH 会重载该配置项
 * （若当前进程没有热加载，重启 DSH 即可）。
 *
 * 用法：
 *   node tools/dsh/install-mcp.mjs
 *   node tools/dsh/install-mcp.mjs --project examples/game_api --editor engine/target/debug/editor.exe
 *   node tools/dsh/install-mcp.mjs --remove
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..');
const BEGIN = '# >>> rustlua-mcp >>>';
const END = '# <<< rustlua-mcp <<<';

function parseArgs(argv) {
  const out = { remove: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--remove') out.remove = true;
    else if (arg === '--project') out.project = argv[++i];
    else if (arg === '--editor') out.editor = argv[++i];
    else if (arg === '--profile') out.profile = argv[++i];
    else if (arg === '--name') out.name = argv[++i];
    else if (arg === '--help' || arg === '-h') out.help = true;
    else throw new Error(`未知参数：${arg}`);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
if (args.help) {
  console.log('用法：node tools/dsh/install-mcp.mjs [--project <工程根>] [--editor <editor.exe>] [--profile <DSH profile 目录>] [--name <serverName>] [--remove]');
  process.exit(0);
}

const profileDir =
  args.profile ??
  process.env.DSH_PROFILE_DIR ??
  path.join(os.homedir(), '.dsh', 'profiles', 'desktop');
const patchPath = path.join(profileDir, 'cordis.patch.yml');
const project = path.resolve(repoRoot, args.project ?? path.join('examples', 'game_api'));
const editor = path.resolve(repoRoot, args.editor ?? path.join('engine', 'target', 'debug', 'editor.exe'));
const serverName = args.name ?? 'rustlua';
const engineDir = path.join(repoRoot, 'engine');

const toPosix = (value) => path.resolve(value).replaceAll('\\', '/');
const quote = (value) => `'${String(value).replaceAll("'", "''")}'`;

function stripBlock(text) {
  const start = text.indexOf(BEGIN);
  const stop = text.indexOf(END);
  if (start === -1 || stop === -1) return text;
  return `${text.slice(0, start)}${text.slice(stop + END.length).replace(/^\r?\n/, '')}`;
}

function renderBlock() {
  return [
    BEGIN,
    '# RustLua 编辑器 MCP：DSH 通过 stdio 拉起 editor --mcp-stdio。',
    '# 只自动放行 T1（含 ui_create_preset）；T2/T3 仍会在编辑器里弹确认窗。',
    '# 必须放在 insert 列表里：mcp-client 不在 dsh-base 的默认插件集中。',
    '- insert:',
    '    - id: mcp-rustlua',
    "      name: '@deepseek-ai/dsh-mcp-client'",
    '      config:',
    `        serverName: ${quote(serverName)}`,
    '        transport: stdio',
    `        command: ${quote(toPosix(editor))}`,
    `        args: ['--project', ${quote(toPosix(project))}, '--mcp-stdio']`,
    `        cwd: ${quote(toPosix(engineDir))}`,
    '        env:',
    "          RUSTLUA_EDITOR_AI_AUTO_T1: '1'",
    '        toolCallTimeoutMs: 120000',
    '        failOnStartupError: false',
    END,
    '',
  ].join('\n');
}

if (!fs.existsSync(profileDir)) {
  console.error(`DSH profile 不存在：${profileDir}`);
  process.exit(2);
}
if (!fs.existsSync(patchPath)) {
  console.error(`找不到 ${patchPath}；请先启动一次 DSH 生成 profile。`);
  process.exit(2);
}

const before = fs.readFileSync(patchPath, 'utf8');
let next = stripBlock(before).replace(/\s+$/, '');
if (next.trim() === '[]') next = '';

if (!args.remove) {
  if (!fs.existsSync(editor)) {
    console.error(`editor.exe 不存在：${editor}\n先在 engine/ 下执行 cargo build -p editor。`);
    process.exit(2);
  }
  if (!fs.existsSync(project)) {
    console.error(`工程目录不存在：${project}`);
    process.exit(2);
  }
  next = `${next}${next ? '\n\n' : ''}${renderBlock()}`;
} else {
  next = `${next}\n`;
}

if (next !== before) {
  const stamp = new Date().toISOString().replaceAll(/[-:T.Z]/g, '').slice(0, 14);
  fs.copyFileSync(patchPath, `${patchPath}.bak-${stamp}`);
  fs.writeFileSync(patchPath, next, 'utf8');
  console.log(`${args.remove ? '已移除' : '已写入'} RustLua MCP 配置：${patchPath}`);
  console.log(`  server=${serverName}  editor=${editor}  project=${project}`);
  console.log('  DSH 会重载该配置项；若工具未出现，重启 DSH。');
} else {
  console.log(`配置未变化：${patchPath}`);
}
