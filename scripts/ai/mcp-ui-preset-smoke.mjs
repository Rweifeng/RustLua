#!/usr/bin/env node
/**
 * MCP「一句话创建 UI 预设」的确定性验收脚本（不经过 LLM）。
 *
 * 它做的是 DSH 接入后同一个 MCP 通道上的最小闭环：
 * initialize → tools/list → resources/read(rustlua://ui/guide) → ui_create_preset
 * → ui_export_spec → 重复创建必须被拒绝。成功输出 `MCP_UI_PRESET_OK`。
 *
 * 用法：
 *   node scripts/ai/mcp-ui-preset-smoke.mjs
 *   node scripts/ai/mcp-ui-preset-smoke.mjs --project examples/game_api --keep
 *
 * 前置：engine/ 下 `cargo build -p editor` 已产出 target/debug/editor.exe。
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..', '..');

function parseArgs(argv) {
  const out = { keep: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--keep') out.keep = true;
    else if (arg === '--project') out.project = argv[++i];
    else if (arg === '--document') out.document = argv[++i];
    else if (arg === '--editor') out.editor = argv[++i];
    else if (arg === '--help' || arg === '-h') out.help = true;
    else throw new Error(`未知参数：${arg}`);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
if (args.help) {
  console.log('用法：node scripts/ai/mcp-ui-preset-smoke.mjs [--project <工程根>] [--document <ui/xxx.ui.json>] [--editor <editor.exe>] [--keep]');
  process.exit(0);
}

const project = path.resolve(repoRoot, args.project ?? path.join('examples', 'game_api'));
const editor = path.resolve(repoRoot, args.editor ?? path.join('engine', 'target', 'debug', 'editor.exe'));
const document = args.document ?? 'ui/mcp_smoke.ui.json';
const file = path.join(project, document);

if (!fs.existsSync(editor)) {
  console.error(`editor.exe 不存在：${editor}（先 cargo build -p editor）`);
  process.exit(2);
}
if (!fs.existsSync(project)) {
  console.error(`工程不存在：${project}`);
  process.exit(2);
}
if (!document.endsWith('.ui.json') || document.includes('..') || path.isAbsolute(document)) {
  console.error(`document 必须是工程内 .ui.json 相对路径：${document}`);
  process.exit(2);
}

const spec = {
  spec_version: 1,
  document: { kind: 'widget', name: 'mcp_smoke' },
  tokens: { 'color.bg': '#101820', 'color.fg': '#f5e6c8' },
  styles: {
    panel: { base: { bg: '@color.bg', radius: 10, padding: [16, 16, 16, 16] } },
    title: { base: { color: '@color.fg', font_size: 24 } },
  },
  root: {
    key: 'root',
    type: 'panel',
    style: 'panel',
    layout: { anchor: 'center', size: [420, 240] },
    children: [
      {
        key: 'title',
        type: 'text',
        text: 'MCP Smoke',
        style: 'title',
        layout: { anchor: 'top_center', position: [0, 20], size: [240, 32] },
      },
      { key: 'ok', type: 'button', text: 'OK', layout: { anchor: 'center', size: [160, 40] } },
    ],
  },
};

const child = spawn(editor, ['--project', project, '--mcp-stdio'], {
  cwd: path.join(repoRoot, 'engine'),
  env: { ...process.env, RUSTLUA_EDITOR_AI_AUTO_T1: '1' },
  stdio: ['pipe', 'pipe', 'pipe'],
});

let buffer = '';
let nextId = 1;
const pending = new Map();
child.stdout.on('data', (chunk) => {
  buffer += chunk.toString();
  let index;
  while ((index = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (!line) continue;
    try {
      const message = JSON.parse(line);
      if (message.id && pending.has(message.id)) {
        pending.get(message.id)(message);
        pending.delete(message.id);
      }
    } catch {
      // 非协议行忽略；stdout 必须只有协议帧。
    }
  }
});
child.stderr.on('data', (chunk) => {
  const text = chunk.toString();
  if (/panic|error/i.test(text)) process.stderr.write(text);
});

function call(method, params, timeoutMs = 60_000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`MCP 超时：${method}`));
    }, timeoutMs);
    pending.set(id, (message) => {
      clearTimeout(timer);
      resolve(message);
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
  });
}

function fail(message) {
  console.error(`MCP_UI_PRESET_FAIL ${message}`);
  child.stdin.end();
  setTimeout(() => {
    child.kill();
    process.exit(1);
  }, 500);
}

try {
  const init = await call('initialize', {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'rustlua-mcp-smoke', version: '1' },
  });
  if (!init.result?.instructions?.includes('rustlua://ui/guide')) {
    fail('initialize.instructions 未指向 rustlua://ui/guide');
  }

  const tools = await call('tools/list', {});
  if (!tools.result?.tools?.some((tool) => tool.name === 'ui_create_preset')) {
    fail('tools/list 缺少 ui_create_preset');
  }

  const resources = await call('resources/list', {});
  if (!resources.result?.resources?.some((resource) => resource.uri === 'rustlua://ui/guide')) {
    fail('resources/list 缺少 rustlua://ui/guide');
  }

  const guide = await call('resources/read', { uri: 'rustlua://ui/guide' });
  if (!guide.result?.contents?.[0]?.text?.includes('ui_create_preset')) {
    fail('ui/guide 未描述 ui_create_preset');
  }

  if (fs.existsSync(file)) {
    fail(`验收文档已存在，拒绝覆盖：${document}`);
  }

  const created = await call('tools/call', {
    name: 'ui_create_preset',
    arguments: { document, spec, preview: true },
  });
  if (created.result?.isError) {
    fail(`ui_create_preset 失败：${created.result.content?.[0]?.text ?? ''}`);
  }
  const payload = created.result.structuredContent;
  // 预览图走 MCP image content block，structuredContent 里只留 revision/元数据。
  const preview = created.result.content?.find((block) => block.type === 'image')?.data;
  if (!payload?.revision || !preview?.startsWith('iVBORw0KGgo')) {
    fail(`回执缺 revision 或 PNG：${JSON.stringify(payload).slice(0, 500)}`);
  }

  const exported = await call('tools/call', {
    name: 'ui_export_spec',
    arguments: { document },
  });
  if (exported.result?.isError) {
    fail(`ui_export_spec 失败：${exported.result.content?.[0]?.text ?? ''}`);
  }
  const roundTrip = exported.result.structuredContent;
  if (String(roundTrip.revision) !== String(payload.revision)) {
    fail(`revision 往返不一致：create=${payload.revision} export=${roundTrip.revision}`);
  }

  const again = await call('tools/call', {
    name: 'ui_create_preset',
    arguments: { document, spec, preview: false },
  });
  if (!again.result?.isError) {
    fail('重复创建未被拒绝');
  }

  if (!args.keep) fs.rmSync(file, { force: true });
  console.log(
    `MCP_UI_PRESET_OK document=${document} revision=${payload.revision} preview_bytes~=${Math.floor(
      (preview.length * 3) / 4,
    )}${args.keep ? ' kept=1' : ' cleaned=1'}`,
  );
  child.stdin.end();
  setTimeout(() => {
    child.kill();
    process.exit(0);
  }, 500);
} catch (error) {
  fail(error.message);
}
