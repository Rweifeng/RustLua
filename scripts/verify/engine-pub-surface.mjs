// WP12 阶段 0 守卫：engine crate 对外 Rust 符号面的基线与比对。
//
// 事实源是 rustdoc 已经生成好的文档（`cargo doc --no-deps --workspace --lib`，verify.ps1 的
// "Rustdoc" 那一步就跑它），不需要 nightly 的 `--output-format json`，也不引入任何依赖。
// 取两层信号：
//   1. 每个文档页面的 `kind + 完整路径`（`struct.Renderer.html` 在 `host/renderer/` 下
//      ⇒ `struct|host::renderer::Renderer`）——含 `pub use` 的重导出位置，所以
//      `host::Renderer` 这类**对外可达路径**也在账上。
//   2. 类型页面上的锚点（`id="method.new"` / `associatedtype` / `associatedconstant`）
//      ⇒ `method|host::renderer::Renderer::new`。拆分时如果把 `impl Renderer` 的一块
//      搬进 `pub mod`（应当是私有子模块），方法就会多出一条新路径，这条门立刻红。
//
// 用法：
//   node scripts/verify/engine-pub-surface.mjs --check    # 与基线比对，不同则退出 1
//   node scripts/verify/engine-pub-surface.mjs --update   # 重新生成基线（只在**有意**改变对外面时用）
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
const docRoot = process.argv[3] || join(repoRoot, 'engine', 'target', 'doc', 'engine');
// 本地基线目录（不入库）：默认 <repoRoot>/baselines，可用 RUSTLUA_BASELINES_DIR 改位置。
const baselinesDir = process.env.RUSTLUA_BASELINES_DIR || join(repoRoot, 'baselines');
const baselinePath = join(baselinesDir, 'engine-pub-surface.json');

if (!existsSync(docRoot)) {
  console.log(`pub_surface_skipped reason=rustdoc_output_missing path=${docRoot} exit=3（先跑 cargo doc --no-deps --workspace --lib）`);
  process.exit(3);
}

const walk = (dir) => {
  const out = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (name.endsWith('.html')) out.push(full);
  }
  return out;
};

const files = walk(docRoot);
if (files.length === 0) {
  console.log('pub_surface_failed reason=no_html（rustdoc 产物是空的）');
  process.exit(1);
}
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
  const path = ['engine', ...modulePath, m[2]].join('::');
  items.push(`${m[1]}|${path}`);
  // 方法锚点只在 `host/` 子树下取：整棵树的锚点里大部分是 blanket/auto trait 的实现
  // （`clone`/`fmt` 之类），它们随依赖版本抖，会把门变成噪声源。本工作包搬的是 host，
  // 需要的是"impl 块搬进私有子模块后方法还挂在同一个类型上"这一条。
  if (!rel.startsWith('host/')) continue;
  for (const anchor of html.matchAll(/id="(method|associatedtype|associatedconstant)\.([A-Za-z0-9_]+)"/g)) {
    items.push(`${anchor[1]}|${path}::${anchor[2]}`);
  }
}
const unique = [...new Set(items)].sort();
if (unique.length === 0) {
  console.log('pub_surface_failed reason=no_items_parsed（rustdoc 版式变了，解析器要跟着改）');
  process.exit(1);
}
// 关键项必须在场：解析器要是漏掉整棵子树，空账也能"逐字节相同"。
for (const must of [
  'struct|engine::host::renderer::Renderer',
  'struct|engine::host::runtime_scene::RuntimeScene',
  'method|engine::host::renderer::Renderer::render',
  'method|engine::host::runtime_scene::RuntimeScene::vertices',
]) {
  if (!unique.includes(must)) {
    console.log(`pub_surface_failed reason=expected_item_missing item=${must} parsed=${unique.length}`);
    process.exit(1);
  }
}

const payload = {
  schema: 'engine-pub-surface/v1',
  generated_by: 'scripts/verify/engine-pub-surface.mjs',
  source: 'cargo doc --no-deps --workspace --lib（engine/target/doc/engine）',
  rustdoc_version: rustdocVersion,
  count: unique.length,
  items: unique,
};
const text = JSON.stringify(payload, null, 2) + '\n';

if (mode === 'update') {
  mkdirSync(dirname(baselinePath), { recursive: true });
  writeFileSync(baselinePath, text, 'utf8');
  console.log(`pub_surface_updated path=${relative(repoRoot, baselinePath).replace(/\\/g, '/')} items=${unique.length}`);
  process.exit(0);
}

if (!existsSync(baselinePath)) {
  console.log(`pub_surface_failed reason=baseline_missing path=${relative(repoRoot, baselinePath).replace(/\\/g, '/')}（用 --update 生成一次）`);
  process.exit(1);
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
const isInternalMove = (item) => {
  const [kind, path] = item.split('|');
  if (!path.startsWith('engine::host::')) return false;
  const segs = path.split('::');
  if (segs.length < 4) return false;
  const name = segs[segs.length - 1];
  if (segs[segs.length - 2] === name) return false;
  return prevSet.has(`${kind}|${[...segs.slice(0, -2), name].join('::')}`);
};
const moved = added.filter(isInternalMove);
const unexplained = added.filter((x) => !isInternalMove(x));

if (unexplained.length === 0 && removed.length === 0) {
  console.log(`pub_surface_ok items=${unique.length} moved_into_submodules=${moved.length}（对外 Rust 符号面与基线一致${moved.length ? '；多出的页面是搬进子模块的定义处，原路径仍可达' : ''}）`);
  process.exit(0);
}
console.log(`pub_surface_failed items=${unique.length} baseline=${prevItems.length} added=${unexplained.length} removed=${removed.length} moved=${moved.length}`);
for (const x of removed.slice(0, 40)) console.log(`  - ${x}`);
for (const x of unexplained.slice(0, 40)) console.log(`  + ${x}`);
console.log('  拆分不得改变对外符号面；确属有意新增/删除才跑 --update 并把理由写进提交说明。');
process.exit(1);
