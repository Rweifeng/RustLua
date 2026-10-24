// WP12 阶段 0 守卫：crate 对外 Rust 符号面的基线与比对。
//
// 事实源是 rustdoc 已经生成好的文档（`cargo doc --no-deps --workspace --lib`，verify.ps1 的
// "Rustdoc" 那一步就跑它），不需要 nightly 的 `--output-format json`，也不引入任何依赖。
// 取两层信号：
//   1. 每个文档页面的 `kind + 完整路径`（`struct.Renderer.html` 在 `renderer/` 下
//      ⇒ `struct|rustlua_gpu::renderer::Renderer`）——含 `pub use` 的重导出位置，所以
//      `engine::host::RuntimeScene` 这类**对外可达路径**也在账上。
//   2. 类型页面上的锚点（`id="method.new"` / `associatedtype` / `associatedconstant`）
//      ⇒ `method|rustlua_gpu::renderer::Renderer::new`。拆分时如果把 `impl Renderer` 的一块
//      搬进 `pub mod`（应当是私有子模块），方法就会多出一条新路径，这条门立刻红。
//
// 用法：
//   node scripts/verify/engine-pub-surface.mjs --check    # 逐 crate 与基线比对，不同则退出 1
//   node scripts/verify/engine-pub-surface.mjs --update   # 重新生成基线（只在**有意**改变对外面时用）
//
// M77-01 起看**两个** crate：`engine`（组合根）与 `rustlua-gpu`（wgpu 实现侧）。
// 渲染器从 engine 搬到独立 crate 是一次**有意的**对外符号面变化（`engine::host::renderer::*`
// 整族消失，改由 `rustlua_gpu::renderer::*` 提供；不留兼容 shim 是批次章程），所以两边的
// 基线都要在同一笔里重刷，否则下一次拆分变更和这次搬迁的 diff 会混在一起读不出来。
//
// 退出码沿用本仓库口径：0=通过；1=判红（或基线缺失）；3=环境受限（没找到 rustdoc 产物，
// 打印原因，绝不把"跳过"说成"通过"）。
//
// 已知边界（记录在 tasks/done/WP12-host拆分.md §14.4 的④）：这份账抓的是**路径与项的种类/归属**，
// 不含函数签名。签名由 `editor`/`game`/`game_web`/`assetc` 的工作区编译与
// `engine/crates/engine/tests/host_pub_surface.rs` 的存在性 pin 一起兜住。
import { readdirSync, readFileSync, existsSync, writeFileSync, statSync, mkdirSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = dirname(dirname(here));
const mode = process.argv[2] === '--update' ? 'update' : 'check';
// 本地基线目录（不入库）：默认 <repoRoot>/baselines，可用 RUSTLUA_BASELINES_DIR 改位置。
const baselinesDir = process.env.RUSTLUA_BASELINES_DIR || join(repoRoot, 'baselines');
const docBase = process.argv[3] || join(repoRoot, 'engine', 'target', 'doc');

// 被盯的 crate。`anchorPrefixes` 限定"在哪些子树下取方法锚点"：整棵树的锚点里大部分是
// blanket/auto trait 的实现（`clone`/`fmt` 之类），它们随依赖版本抖，会把门变成噪声源。
// WP12 搬的是 `engine::host`，M77-01 搬的是渲染器，两边都只需要"impl 块搬进私有子模块后
// 方法还挂在同一个类型上"这一条。
const WATCHED = [
  {
    name: 'engine',
    baseline: 'engine-pub-surface.json',
    anchorPrefixes: ['host/'],
    must: [
      'struct|engine::host::runtime_scene::RuntimeScene',
      'method|engine::host::runtime_scene::RuntimeScene::vertices',
    ],
  },
  {
    name: 'rustlua_gpu',
    baseline: 'rustlua-gpu-pub-surface.json',
    anchorPrefixes: ['renderer/'],
    must: [
      'struct|rustlua_gpu::renderer::Renderer',
      'method|rustlua_gpu::renderer::Renderer::render',
    ],
  },
];

const walk = (dir) => {
  const out = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (name.endsWith('.html')) out.push(full);
  }
  return out;
};

const collect = (watched) => {
  const docRoot = join(docBase, watched.name);
  if (!existsSync(docRoot)) {
    return { skipped: `rustdoc_output_missing path=${docRoot.replace(/\\/g, '/')}` };
  }
  const files = walk(docRoot);
  if (files.length === 0) return { failed: 'no_html（rustdoc 产物是空的）' };
  const items = [];
  let rustdocVersion = '';
  for (const file of files) {
    const rel = relative(docRoot, file).replaceAll('\\', '/');
    const html = readFileSync(file, 'utf8');
    const vm = /data-rustdoc-version="([^"]+)"/.exec(html);
    if (vm) rustdocVersion = vm[1];
    if (rel === 'index.html' || rel === 'all.html') continue;
    const m = /^(constant|fn|struct|enum|trait|type|union|macro|primitive|derive)\.(.+)\.html$/.exec(rel.split('/').pop());
    if (!m) continue;
    const modulePath = rel.split('/').slice(0, -1);
    const path = [watched.name, ...modulePath, m[2]].join('::');
    items.push(`${m[1]}|${path}`);
    if (!watched.anchorPrefixes.some((prefix) => rel.startsWith(prefix))) continue;
    for (const anchor of html.matchAll(/id="(method|associatedtype|associatedconstant)\.([A-Za-z0-9_]+)"/g)) {
      items.push(`${anchor[1]}|${path}::${anchor[2]}`);
    }
  }
  const unique = [...new Set(items)].sort();
  if (unique.length === 0) return { failed: 'no_items_parsed（rustdoc 版式变了，解析器要跟着改）' };
  // 关键项必须在场：解析器要是漏掉整棵子树，空账也能"逐字节相同"。
  for (const must of watched.must) {
    if (!unique.includes(must)) return { failed: `expected_item_missing item=${must} parsed=${unique.length}` };
  }
  return { items: unique, rustdocVersion };
};

let exitCode = 0;
for (const watched of WATCHED) {
  const baselinePath = join(baselinesDir, watched.baseline);
  const found = collect(watched);
  if (found.skipped) {
    console.log(`pub_surface_skipped crate=${watched.name} reason=${found.skipped} exit=3（先跑 cargo doc --no-deps --workspace --lib）`);
    exitCode = Math.max(exitCode, 3);
    continue;
  }
  if (found.failed) {
    console.log(`pub_surface_failed crate=${watched.name} reason=${found.failed}`);
    exitCode = Math.max(exitCode, 1);
    continue;
  }
  const unique = found.items;
  const payload = {
    schema: 'engine-pub-surface/v2',
    generated_by: 'scripts/verify/engine-pub-surface.mjs',
    source: `cargo doc --no-deps --workspace --lib（${watched.name}）`,
    crate: watched.name,
    rustdoc_version: found.rustdocVersion,
    count: unique.length,
    items: unique,
  };
  const text = JSON.stringify(payload, null, 2) + '\n';

  if (mode === 'update') {
    mkdirSync(dirname(baselinePath), { recursive: true });
    writeFileSync(baselinePath, text, 'utf8');
    console.log(`pub_surface_updated crate=${watched.name} path=${relative(repoRoot, baselinePath).replace(/\\/g, '/')} items=${unique.length}`);
    continue;
  }

  if (!existsSync(baselinePath)) {
    console.log(`pub_surface_failed crate=${watched.name} reason=baseline_missing path=${relative(repoRoot, baselinePath).replace(/\\/g, '/')}（用 --update 生成一次）`);
    exitCode = Math.max(exitCode, 1);
    continue;
  }
  const before = readFileSync(baselinePath, 'utf8').replace(/^\uFEFF/, '');
  const prevItems = JSON.parse(before).items ?? [];
  const prevSet = new Set(prevItems);
  const nextSet = new Set(unique);
  const added = unique.filter((x) => !prevSet.has(x));
  const removed = prevItems.filter((x) => !nextSet.has(x));

  // WP12 拆分带来的**唯一**一类合法新增：一个 `pub` 项从 `engine::host::x::NAME`
  // 搬进 `x` 的子模块 `sub`（⇒ `engine::host::x::sub::NAME`），同时 `x/mod.rs` 用
  // `pub use` 把原路径留着。这时基线里的原路径**必须仍然在场**（由下面的
  // `removed.length === 0` 兜住），新增那条只是"同一个符号的定义处页面"，不是新的对外 API。
  // 判据刻意收紧到"只多一段模块名、且去掉那一段就是基线里的项"：凭空多出来的 pub 项、
  // 或者一次搬进两层深（说明结构变化已超出"纯搬运"）都照判红。
  const modulePrefix = `${watched.name}::`;
  const isInternalMove = (item) => {
    const [kind, path] = item.split('|');
    if (!path.startsWith(modulePrefix)) return false;
    const segs = path.split('::');
    if (segs.length < 4) return false;
    const name = segs[segs.length - 1];
    if (segs[segs.length - 2] === name) return false;
    return prevSet.has(`${kind}|${[...segs.slice(0, -2), name].join('::')}`);
  };
  const moved = added.filter(isInternalMove);
  const unexplained = added.filter((x) => !isInternalMove(x));

  if (unexplained.length === 0 && removed.length === 0) {
    console.log(`pub_surface_ok crate=${watched.name} items=${unique.length} moved_into_submodules=${moved.length}（对外 Rust 符号面与基线一致${moved.length ? '；多出的页面是搬进子模块的定义处，原路径仍可达' : ''}）`);
    continue;
  }
  console.log(`pub_surface_failed crate=${watched.name} items=${unique.length} baseline=${prevItems.length} added=${unexplained.length} removed=${removed.length} moved=${moved.length}`);
  for (const x of removed.slice(0, 40)) console.log(`  - ${x}`);
  for (const x of unexplained.slice(0, 40)) console.log(`  + ${x}`);
  console.log('  拆分不得改变对外符号面；确属有意新增/删除才跑 --update 并把理由写进提交说明。');
  exitCode = Math.max(exitCode, 1);
}
process.exit(exitCode);
