#!/usr/bin/env node
/**
 * 3D 推箱子工程上的 MCP/AI 能力验收流水线（不经 LLM，逐条确定性调用）。
 *
 * 被测对象是 `editor --project <工程> --mcp-stdio` 这条真实 MCP 通道，按
 *   协议面 → T0 只读 → T1 场景写 → T2/T3 确认门 → 落盘对账
 * 五档逐条真调，每一步的返回或报错原文都进报告。
 *
 * 读数口径（四类，不合并）：
 *   ok      通道真的跑通了
 *   gate    被确认门拦住，等人工批准（T2/T3 的设计行为，不是缺陷）
 *   absent  引擎/AI 面没有这个能力
 *   err     意外失败（计入 failures）
 *
 * 用法：
 *   node scripts/ai/mcp-3d-capability-probe.mjs
 *   node scripts/ai/mcp-3d-capability-probe.mjs --wait-approval 240   # 排队后在编辑器窗口点批准
 *   node scripts/ai/mcp-3d-capability-probe.mjs --skip-writes
 *
 * 前置：`cargo build -p editor` 已产出 engine/target/debug/editor.exe。
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

function parseArgs(argv) {
  const out = { waitApproval: 0, editWarmup: '3000' };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--project') out.project = argv[++i];
    else if (a === '--editor') out.editor = argv[++i];
    else if (a === '--out') out.out = argv[++i];
    else if (a === '--wait-approval') out.waitApproval = Number(argv[++i]);
    else if (a === '--skip-writes') out.skipWrites = true;
    else if (a === '--boot-scene') out.bootScene = true;
    else if (a === '--edit-scene') out.editScene = argv[++i];
    else if (a === '--edit-warmup') out.editWarmup = argv[++i];
    else if (a === '--help' || a === '-h') out.help = true;
    else throw new Error(`未知参数：${a}`);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
if (args.help) {
  console.log('用法：node scripts/ai/mcp-3d-capability-probe.mjs [--project <工程根>] [--editor <editor.exe>] [--out <json>] [--wait-approval <秒>] [--skip-writes] [--boot-scene]');
  process.exit(0);
}

const project = path.resolve(repoRoot, args.project ?? path.join('examples', 'game_sokoban3d'));
const editor = path.resolve(repoRoot, args.editor ?? path.join('engine', 'target', 'debug', 'editor.exe'));
const outFile = path.resolve(repoRoot, args.out ?? path.join('.temp', 'mcp3d-probe.json'));
const sceneRel = 'scenes/main.scene';

const report = [];
let failures = 0;

function record(kind, step, detail) {
  report.push({ kind, step, detail: String(detail ?? '') });
  console.log(`PROBE ${kind.padEnd(6)} ${step.padEnd(36)} ${String(detail ?? '').replace(/\s+/g, ' ').slice(0, 170)}`);
}

function classify(text) {
  if (/QueuedForConfirmation|已在编辑器.*排队等待人工批准/i.test(text)) return 'gate';
  if (/不可用/i.test(text)) return 'absent';
  if (/unknown tool/i.test(text)) return 'absent';
  return null;
}

function note(ok, step, text) {
  if (ok) {
    record('ok', step, text);
    return true;
  }
  const kind = classify(text);
  if (kind) {
    record(kind, step, text);
    return false;
  }
  failures += 1;
  record('err', step, text);
  return false;
}

// 进场景编辑态的两条不经人工批准的通道（都靠编辑器自己的调试钩子，不是引擎能力）：
//   --boot-scene  autoplay：开机打开场景，但同时强制 Play —— 而 Play 态下 T1 场景写被
//                 command_router.rs:215 硬拒，所以这条只能跑读面，跑不了写面。
//   --edit-scene  GUI 证据运行器的准备阶段：open_scene_resource 后停在编辑态，且每步
//                 等 WARMUP 帧才截图；把 warmup 调大就换来一段可写的编辑窗口。
const evidenceDir = args.editScene ? path.resolve(repoRoot, args.editScene) : null;
const child = spawn(editor, ['--project', project, '--mcp-stdio'], {
  cwd: path.resolve(repoRoot, 'engine'),
  env: {
    ...process.env,
    RUSTLUA_EDITOR_AI_AUTO_T1: '1',
    ...(args.bootScene ? { RUSTLUA_EDITOR_AUTOPLAY: '1', RUSTLUA_EDITOR_AUTOPLAY_SCENE: sceneRel } : {}),
    ...(evidenceDir ? {
      RUSTLUA_EDITOR_GUI_EVIDENCE: evidenceDir,
      RUSTLUA_EDITOR_EVIDENCE_SCENE: sceneRel,
      RUSTLUA_EDITOR_GUI_EVIDENCE_WARMUP: args.editWarmup,
    } : {}),
  },
  stdio: ['pipe', 'pipe', 'pipe'],
});
let stderr = '';
child.stderr.on('data', (d) => { stderr += d.toString(); });

let buf = '';
const pending = new Map();
let nextId = 1;
child.stdout.on('data', (data) => {
  buf += data.toString();
  let idx;
  while ((idx = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, idx).trim();
    buf = buf.slice(idx + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id).resolve(msg);
      pending.delete(msg.id);
    }
  }
});

function rpc(method, params, timeoutMs = 60000) {
  const id = nextId += 1;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`timeout: ${method}`));
    }, timeoutMs);
    pending.set(id, { resolve: (msg) => { clearTimeout(timer); resolve(msg); } });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function tool(name, input, timeoutMs = 60000) {
  try {
    const msg = await rpc('tools/call', { name, arguments: input ?? {} }, timeoutMs);
    if (msg.error) return { ok: false, text: `${msg.error.code} ${msg.error.message}` };
    const result = msg.result ?? {};
    const text = (result.content ?? []).map((item) => (item.type === 'text' ? item.text : `[${item.type}]`)).join(' ');
    const structured = result.structuredContent ? JSON.stringify(result.structuredContent) : '';
    return { ok: !result.isError, text: `${text}${structured ? ` ${structured}` : ''}`.trim() };
  } catch (error) {
    return { ok: false, text: String(error?.message ?? error) };
  }
}

async function read(uri) {
  try {
    const msg = await rpc('resources/read', { uri });
    if (msg.error) return { ok: false, text: `${msg.error.code} ${msg.error.message}` };
    return { ok: true, text: (msg.result?.contents ?? []).map((c) => c.text ?? `[binary ${c.mimeType}]`).join('\n') };
  } catch (error) {
    return { ok: false, text: String(error?.message ?? error) };
  }
}

/// 等人工在编辑器窗口里批准：轮询一个"批准后才成立"的观测，而不是猜时间。
async function waitFor(label, probe, timeoutSec) {
  if (timeoutSec <= 0) return false;
  const deadline = Date.now() + timeoutSec * 1000;
  let tick = 0;
  while (Date.now() < deadline) {
    await sleep(3000);
    tick += 1;
    if (await probe()) {
      record('ok', `${label} 人工批准后成立`, `等待 ${tick * 3}s`);
      return true;
    }
    if (tick === 3) {
      console.log(`AWAITING_APPROVAL ${label} —— 请到编辑器窗口「AI 动作确认」里点批准（剩 ${Math.round((deadline - Date.now()) / 1000)}s）`);
    }
  }
  record('gate', `${label} 未获批准`, `等待 ${timeoutSec}s 超时，保持排队/未执行`);
  return false;
}

const idFrom = (text) => {
  const match = /"new_entities"\s*:\s*\[\s*(\d+)/.exec(text) ?? /"entity"\s*:\s*(\d+)/.exec(text) ?? /"new_entities"\s*:\s*\[\s*\d+\s*,\s*(\d+)/.exec(text);
  return match ? Number(match[1]) : null;
};

// ── A：协议与发现面 ─────────────────────────────────────────────────────────────
const init = await rpc('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'mcp-3d-capability-probe', version: '1' } });
note(!init.error, 'initialize', init.result?.serverInfo?.name ?? `${init.error?.code} ${init.error?.message}`);
record('info', 'initialize.instructions', `${(init.result?.instructions ?? '').length} chars`);
const toolsList = (await rpc('tools/list', {})).result?.tools ?? [];
record('info', 'tools/list', `${toolsList.length} tools`);
const resourcesList = (await rpc('resources/list', {})).result?.resources ?? [];
record('info', 'resources/list', `${resourcesList.length} resources`);
record('absent', 'prompts/list', resourcesList.length ? 'MCP 只暴露 resources，无 prompts 面' : 'no prompts');

for (const res of resourcesList) {
  const got = await read(res.uri);
  const mentions3d = /MeshRenderer|Collider3d|perspective/i.test(got.text);
  note(got.ok, `resources/read ${res.uri}`, got.ok ? `${got.text.length} chars, 3D 内容=${mentions3d}` : got.text);
}

// ── B：T0 只读面（真工程） ──────────────────────────────────────────────────────
const cfg = await tool('get_project_config');
note(cfg.ok, 'get_project_config', cfg.ok ? `project.toml 可读（${cfg.text.length} chars）` : cfg.text);

let tree = await tool('get_scene_tree');
if (!tree.ok) {
  // 打开场景本身是 T3：这是"AI 进不了场景"的第一道结构性墙。
  record('absent', 'editor 启动即打开 startup_scene', `get_scene_tree 直接被拒：${tree.text}`);
  if (evidenceDir) {
    const opened = await waitFor('证据运行器准备阶段开场景（编辑态）', async () => {
      tree = await tool('get_scene_tree');
      return tree.ok;
    }, Math.max(args.waitApproval, 150));
    record(opened ? 'ok' : 'err', '编辑态场景可达（无需人工批准）', opened
      ? '证据运行器的开机开场景 + 不 Play ⇒ AI 的 T0/T1 面全部可达'
      : `准备阶段没打开场景：${tree.text}`);
    if (!opened) failures += 1;
  } else if (args.bootScene) {
    const opened = await waitFor('autoplay 开机开场景', async () => {
      tree = await tool('get_scene_tree');
      return tree.ok;
    }, Math.max(args.waitApproval, 30));
    record(opened ? 'ok' : 'err', 'autoplay 绕过 T3 进入场景编辑态', opened
      ? '编辑器的开机钩子打开场景后，AI 的 T0 面可达（但 autoplay 同时强制 Play，写面另有实验）'
      : `autoplay 也没打开场景：${tree.text}`);
    if (!opened) failures += 1;
  } else {
    const opened = await tool('open_scene', { path: sceneRel });
    note(opened.ok, 'open_scene (T3)', opened.text);
    if (!opened.ok) {
      const approved = await waitFor('open_scene', async () => (await tool('get_scene_tree')).ok, args.waitApproval);
      if (approved) tree = await tool('get_scene_tree');
    } else {
      tree = await tool('get_scene_tree');
    }
  }
}
note(tree.ok, 'get_scene_tree', tree.ok ? `${tree.text.length} chars` : tree.text);

if (tree.ok) {
  const hasPlayer = /\bPlayer\b/.test(tree.text);
  const hasFloor = /\bFloor\b/.test(tree.text);
  record(hasPlayer && hasFloor ? 'ok' : 'err', 'scene 3D 实可见', `Player=${hasPlayer} Floor=${hasFloor}`);
  if (!(hasPlayer && hasFloor)) failures += 1;
}
// Floor 的 id：JSON 里 id/name 的先后次序不保证，两种都试；同时把整棵树落一份，
// 便于报告里逐实体对账（33 个实体是不是真在编辑态里）。
if (tree.ok) fs.writeFileSync(path.resolve(repoRoot, '.temp', 'mcp3d_scene_tree.json'), tree.text);
const floorMatch = /"id"\s*:\s*(\d+),"name"\s*:\s*"Floor"/.exec(tree.text)
  ?? /"name"\s*:\s*"Floor"[^{}]*?"id"\s*:\s*(\d+)/.exec(tree.text);
const floorId = floorMatch ? Number(floorMatch[1]) : null;

const comps = await tool('list_components');
note(comps.ok, 'list_components', comps.ok ? `${comps.text.length} chars` : comps.text);
for (const want of ['MeshRenderer', 'Collider3d', 'Camera', 'Light', 'Environment', 'Animation3D']) {
  const hit = new RegExp(`\\b${want}\\b`).test(comps.text);
  record(hit ? 'ok' : 'absent', `list_components: ${want}`, hit ? '在注册表面里' : '不在 list_components 输出里');
}

for (const component of ['MeshRenderer', 'Collider3d', 'Camera', 'Light', 'Environment', 'Animation3D']) {
  const schema = await tool('get_inspector_schema', { component, ...(floorId ? { entity: floorId } : {}) });
  note(schema.ok, `get_inspector_schema ${component}`, schema.ok ? `${schema.text.length} chars` : schema.text);
}

const consoleRead = await tool('read_console');
note(consoleRead.ok, 'read_console', consoleRead.ok ? `${consoleRead.text.length} chars` : consoleRead.text);

const apiSurface = await tool('list_lua_api');
for (const mod of ['camera3d', 'transform3d', 'physics3d', 'animation3d', 'mesh', 'light']) {
  const hit = new RegExp(`\\b${mod}\\b`).test(apiSurface.text);
  note(hit, `list_lua_api: ${mod}`, hit ? 'present' : `期望 present 但没有：${apiSurface.text.slice(0, 80)}`);
}
for (const mod of ['material', 'environment', 'model', 'fog']) {
  const hit = new RegExp(`\\b${mod}\\b`).test(apiSurface.text);
  record(hit ? 'present' : 'absent', `Lua surface: ${mod}`, hit ? 'unexpectedly present' : 'no such module');
}

const evalProbe = await tool('eval_lua_readonly', { expression: 'return app.platform()' });
note(evalProbe.ok, 'eval_lua_readonly', evalProbe.text);

// 只读 VM 里有 API 表，但没有运行中的世界：这条测的是"AI 能不能用 eval 读活体场景"。
const evalScene = await tool('eval_lua_readonly', {
  expression: 'local ok, res, err = pcall(function() return transform3d.get(1) end) return tostring(ok) .. "|" .. tostring(res) .. "|" .. tostring(err)',
});
const seesWorld = evalScene.ok && /true\|table/.test(evalScene.text);
record(seesWorld ? 'ok' : 'absent', 'eval_lua_readonly 能读活体场景吗', evalScene.ok
  ? `pcall 结果=${evalScene.text}（只读 VM 与游戏运行态隔离：编辑态的 33 个实体不在里面）`
  : evalScene.text);

const shot = await tool('capture_screenshot');
note(shot.ok, 'capture_screenshot', shot.text);

// ── C：T1 写面（AI 真改 3D 场景） ───────────────────────────────────────────────
let probeEntity = null;
if (!args.skipWrites && tree.ok) {
  // 开机后首条写会被还在铺的资产刷新吞掉（Rejected: 资源刷新完成：N 项）：
  // 重试而不是把它算成缺陷，同时记下重试了几遍。
  let created = null;
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    created = await tool('create_entity', { name: 'McpProbeCrate', transform_3d: true });
    if (created.ok) {
      record(attempt === 1 ? 'ok' : 'ok', 'create_entity(transform_3d)', `第 ${attempt} 次成功：${created.text}`);
      break;
    }
    if (attempt === 4) note(false, 'create_entity(transform_3d)', created.text);
    else await sleep(1500);
  }
  probeEntity = idFrom(created.text);

  if (probeEntity != null) {
    const addMesh = await tool('add_component', { entity: probeEntity, component: 'MeshRenderer' });
    note(addMesh.ok, 'add_component MeshRenderer', addMesh.text);
    const addCol = await tool('add_component', { entity: probeEntity, component: 'Collider3d' });
    note(addCol.ok, 'add_component Collider3d', addCol.text);

    const meshWrite = await tool('set_component_fields', {
      entities: [probeEntity], component: 'MeshRenderer',
      fields: { mesh: 'assets/models/sokoban-cube.glb', material: 'assets/materials/crate.material' },
    });
    note(meshWrite.ok, 'set_component_fields MeshRenderer', meshWrite.text);
    // 字段依赖顺序实测：set_component_fields 把 fields 交给 serde_json 的 BTreeMap，
    // 引擎解析器又按字母序逐字段应用（physics3d.rs:202-229），所以 shape=cuboid 排在
    // half_* 之后必挂。这里分两条写，并把"顺序敏感"本身记成一条能力缺口。
    const colShape = await tool('set_component_fields', {
      entities: [probeEntity], component: 'Collider3d', fields: { shape: 'cuboid' },
    });
    note(colShape.ok, 'set_component_fields Collider3d.shape', colShape.text);
    const colWrite = await tool('set_component_fields', {
      entities: [probeEntity], component: 'Collider3d',
      fields: { half_width: '0.46', half_height: '0.46', half_z: '0.46', dynamic: 'false' },
    });
    note(colWrite.ok, 'set_component_fields Collider3d 尺寸', colWrite.text);
    const oneShot = await tool('set_component_fields', {
      entities: [probeEntity], component: 'Collider3d',
      fields: { shape: 'ball', radius: '0.5' },
    });
    record(oneShot.ok ? 'ok' : 'absent', '同批写 shape+radius 的顺序敏感性', oneShot.ok
      ? `字母序侥幸可用（radius 排在 shape 之后）：${oneShot.text.slice(0, 60)}`
      : `一条 tool 调用内无法同时写有依赖的字段（AI 提交的顺序被丢掉）：${oneShot.text.slice(0, 120)}`);
    const trWrite = await tool('set_component_fields', {
      entities: [probeEntity], component: 'Transform', fields: { position: '3.4,0.5,-2.6' },
    });
    note(trWrite.ok, 'set_component_fields Transform', trWrite.text);

    const back = await tool('get_inspector_schema', { component: 'MeshRenderer', entity: probeEntity });
    const matches = back.text.includes('sokoban-cube');
    record(matches ? 'ok' : 'err', 'MCP 写入回读对账', matches ? 'MeshRenderer.mesh 读回 = sokoban-cube.glb' : `读不到写入值：${back.text.slice(0, 120)}`);
    if (!matches) failures += 1;

    const dup = await tool('duplicate_entity', { entity: probeEntity });
    note(dup.ok, 'duplicate_entity', dup.text);
    const dupId = idFrom(dup.text);
    if (dupId != null) {
      const rn = await tool('rename_entity', { entity: dupId, name: 'McpProbeCrateCopy' });
      note(rn.ok, 'rename_entity', rn.text);
      const sel = await tool('select_entity', { id: dupId });
      note(sel.ok, 'select_entity', sel.text);
      if (floorId == null) {
        record('gate', 'reparent_entity', '场景树里没解析出 Floor 的 id（见 .temp/mcp3d_scene_tree.json），跳过移动');
      } else {
        const rep = await tool('reparent_entity', { entity: dupId, target: floorId, position: 'child' });
        note(rep.ok, `reparent_entity → Floor#${floorId}`, rep.text);
      }
    }
    const undoRes = await tool('undo', {});
    note(undoRes.ok, 'undo (T1)', undoRes.text);
    const redoRes = await tool('redo', {});
    note(redoRes.ok, 'redo (T1)', redoRes.text);

    // Prefab 面：45 个 tool 里 6 个属于 prefab，但没有一个能"造出/放下一个实例"。
    // 这里用非实例实体去撞这 6 条，量的是"接口在、但 AI 拿不到合法输入"。
    const reapply = await tool('reapply_prefab', { entity: probeEntity });
    record(reapply.ok ? 'ok' : 'absent', 'reapply_prefab (T1)', reapply.ok
      ? reapply.text
      : `6 条 prefab tool 都只吃"已存在的实例 id"，工程里没有实例时全部不可达：${reapply.text.slice(0, 80)}`);
    const unpack = await tool('unpack_prefab', { entity: probeEntity });
    note(unpack.ok, 'unpack_prefab (T2)', unpack.text);
    const mkPrefab = await tool('save_prefab_to_source', { entity: probeEntity });
    note(mkPrefab.ok, 'save_prefab_to_source (T2)', mkPrefab.text);

    // T2：破坏性写。
    const del = await tool('delete_entity', { entities: [probeEntity] });
    note(del.ok, 'delete_entity (T2)', del.text);
    const lua = await tool('write_lua_script', { path: 'scripts/mcp_probe.lua', code: 'return { ok = true }\n' });
    note(lua.ok, 'write_lua_script (T2)', lua.text);
    if (lua.ok) fs.rmSync(path.join(project, 'scripts', 'mcp_probe.lua'), { force: true });

    // T3：会话级写——存盘 / Play / 切工程。
    const save = await tool('save_scene', {});
    note(save.ok, 'save_scene (T3)', save.text);
    const play = await tool('set_play_state', { action: 'start' });
    note(play.ok, 'set_play_state start (T3)', play.text);
    const stepped = await tool('step_play', {});
    note(stepped.ok, 'step_play (T3)', stepped.text);
    const stopped = await tool('set_play_state', { action: 'stop' });
    note(stopped.ok, 'set_play_state stop (T3)', stopped.text);
    // 切工程：路径用 `.`（同一个工程）——真要切走会把后面的存盘对账带到别的工程去。
    const switchProject = await tool('open_project', { path: '.' });
    note(switchProject.ok, 'open_project (T3, 同工程)', switchProject.text);

    // 人工批准之后：场景真的写进磁盘了吗？
    if (!save.ok) {
      await waitFor('save_scene', async () => {
        const text = fs.existsSync(path.join(project, sceneRel)) ? fs.readFileSync(path.join(project, sceneRel), 'utf8') : '';
        return text.includes('McpProbeCrate');
      }, args.waitApproval);
    }
  }
}

// UI 预设：AI 侧唯一已跑通的"一句话落盘"通道，在本工程复跑（不依赖场景是否打开）。
const uiSpec = {
  spec_version: 1,
  document: { kind: 'widget', name: 'mcp3d_hud' },
  tokens: { 'color.bg': '#0d1218', 'color.fg': '#e8f0ff' },
  styles: { panel: { base: { bg: '@color.bg', radius: 8, padding: [12, 12, 12, 12] } }, title: { base: { color: '@color.fg', font_size: 20 } } },
  root: {
    key: 'root', type: 'panel', style: 'panel', layout: { anchor: 'top_left', position: [24, 24], size: [320, 120] },
    children: [
      { key: 'title', type: 'text', text: '3D Sokoban', style: 'title', layout: { anchor: 'top_left', position: [12, 12], size: [280, 28] } },
      { key: 'status', type: 'text', text: 'moves 0 / pushes 0', layout: { anchor: 'top_left', position: [12, 48], size: [280, 24] } },
    ],
  },
};
const validate = await tool('ui_validate_spec', { spec: uiSpec });
note(validate.ok, 'ui_validate_spec', validate.text);
const hudRel = 'ui/mcp3d_hud.ui.json';
// create 只允许新建：复跑先删掉上一轮的产物（探针自己的输出，不是工程内容）。
if (!args.skipWrites) fs.rmSync(path.join(project, hudRel), { force: true });
const preset = await tool('ui_create_preset', {
  document: hudRel,
  preview: false,
  spec: uiSpec,
});
note(preset.ok, 'ui_create_preset', preset.text);

// ── E：UI 生成链（读回 → 修改 → 出图 → 切图 → 审阅）────────────────────────────
const exported = await tool('ui_export_spec', { document: hudRel });
record(exported.ok ? 'ok' : 'gate', 'ui_export_spec 读回', exported.ok
  ? `revision=${(/"revision":(\d+)/.exec(exported.text) ?? [])[1] ?? '?'}，${exported.text.length} chars`
  : exported.text);
const revision = Number((/"revision":(\d+)/.exec(exported.text) ?? [])[1] ?? '0');
const patchRes = await tool('ui_apply_patch', {
  document: hudRel,
  patch: {
    base_revision: revision,
    ops: [{ op: 'replace', key: 'status', path: '/text', value: 'moves 3 / pushes 1' }],
  },
});
note(patchRes.ok, 'ui_apply_patch 改文案', patchRes.ok
  ? `${patchRes.text.slice(0, 100)}（回执是 proposal：要人工接受才开事务）`
  : patchRes.text);
const stalePatch = await tool('ui_apply_patch', {
  document: hudRel,
  patch: {
    base_revision: revision === 0 ? 999999 : revision - 1,
    ops: [{ op: 'replace', key: 'status', path: '/text', value: '过期修订号' }],
  },
});
record(stalePatch.ok ? 'err' : 'ok', '过期 base_revision 被拒', stalePatch.ok
  ? `居然接受了过期修订：${stalePatch.text.slice(0, 90)}`
  : `按设计拒绝：${stalePatch.text.slice(0, 90)}`);
if (stalePatch.ok) failures += 1;

const generated = await tool('ui_generate_assets', {
  plan: [{
    key: 'mcp3d_hud_bg',
    role: 'background',
    generate: { prompt: 'dark navy HUD panel, subtle grid', size: [256, 256], seed: 7 },
  }],
});
note(generated.ok, 'ui_generate_assets 本地确定性出图', generated.ok
  ? `${generated.text.length} chars 回执`
  : generated.text);
const genPng = path.join(project, 'assets', 'generated', 'ui', 'mcp3d_hud_bg.png');
record(fs.existsSync(genPng) ? 'ok' : 'gate', '生成资产落盘', fs.existsSync(genPng)
  ? `assets/generated/ui/mcp3d_hud_bg.png = ${fs.statSync(genPng).size} bytes`
  : 'PNG 未落盘（回执带字节，是否写工程取决于 store 配置）');

const sliced = await tool('ui_slice_asset', {
  source: 'assets/generated/ui/mcp3d_hud_bg.png',
  output_dir: 'assets/generated/ui/sliced',
  regions: [{ name: 'top_left', rect: [0, 0, 128, 128] }],
});
note(sliced.ok, 'ui_slice_asset 切图', sliced.ok ? sliced.text.slice(0, 140) : sliced.text);
const slicesManifest = path.join(project, 'assets', 'generated', 'ui', 'sliced', 'slices.json');
record(fs.existsSync(slicesManifest) ? 'ok' : 'gate', '切图清单落盘', fs.existsSync(slicesManifest)
  ? 'slices.json 存在'
  : `切片回执 ok=${sliced.ok}，但 slices.json 不在${args.skipWrites ? '（本轮跳过写）' : ''}`);

const promoted = await tool('ui_promote_asset', {
  asset_id: 'mcp3d_hud_bg',
  reviewed: true,
  license: 'local-checker',
  used_by: [hudRel],
});
note(promoted.ok, 'ui_promote_asset 审阅登记', promoted.ok ? promoted.text.slice(0, 140) : promoted.text);

const assets = await tool('ui_list_assets', {});
const listed = assets.ok && /mcp3d_hud_bg/.test(assets.text);
record(listed ? 'ok' : (assets.ok ? (generated.ok ? 'err' : 'gate') : 'gate'),
  'ui_list_assets 回读 provenance', assets.ok
    ? `${assets.text.length} chars，含生成资产=${listed}${generated.ok ? '' : '（出图被 T2 门拦住，登记面本来就没有这条）'}`
    : assets.text);
if (assets.ok && generated.ok && !listed) failures += 1;

const preview = await tool('ui_preview_spec', { spec: uiSpec, width: 360, height: 120, scale: 1 });
note(preview.ok, 'ui_preview_spec 离屏自检', preview.ok
  ? `${preview.text.length} chars（PNG base64 在 MCP 层的编码情况）`
  : preview.text);

// ── D：落盘对账 ─────────────────────────────────────────────────────────────────
const sceneFile = path.join(project, sceneRel);
const onDisk = fs.existsSync(sceneFile) ? fs.readFileSync(sceneFile, 'utf8') : '';
const persisted = onDisk.includes('McpProbeCrate');
record(persisted ? 'ok' : 'gate', 'MCP T1 写入是否落盘', persisted
  ? `${sceneRel} 含 McpProbeCrate：AI 建的 3D 实体进了工程文件`
  : `${sceneRel} 不含 McpProbeCrate：T1 只改内存文档，存盘这一步是 T3 人工确认`);
const hudOnDisk = fs.existsSync(path.join(project, hudRel));
record(hudOnDisk ? 'ok' : 'err', 'ui_create_preset 落盘', hudOnDisk ? `${hudRel} 存在` : '文件不存在');
if (!hudOnDisk && !args.skipWrites) failures += 1;

const counts = report.reduce((acc, r) => { acc[r.kind] = (acc[r.kind] ?? 0) + 1; return acc; }, {});
record('info', '计数', Object.entries(counts).map(([k, v]) => `${k}=${v}`).join(' '));
if (stderr.trim()) record('info', 'editor stderr', `${stderr.trim().length} chars`);

fs.mkdirSync(path.dirname(outFile), { recursive: true });
fs.writeFileSync(outFile, `${JSON.stringify({ project, failures, counts, report }, null, 2)}\n`);
console.log(`report written: ${outFile}`);
console.log(failures === 0 ? 'MCP_3D_PROBE_OK' : `MCP_3D_PROBE_FAILED failures=${failures}`);

child.stdin.end();
setTimeout(() => { child.kill(); process.exit(failures === 0 ? 0 : 1); }, 800);
