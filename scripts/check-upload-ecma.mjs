// 微信**上传器**的代码包 JS 解析器比开发者工具低一档。实测（2026-10-03，稳定版
// 2.02.2608070，appid wx7091f1cf872130bb）同一台机器上两次上传被拒，报的都是
// `Error: invalid file: <某个 .js>, <行>:<列> SyntaxError: Unexpected token ?`——同一份包在
// 开发者工具里跑得好好的、一帧不缺，只有"上传"这一步炸，所以必须做成发布期能机的门。
//
// 判据：用 acorn 按上传器实测接受的那一档（`2019`）以**脚本模式**解析包内每个 .js。
// 为什么是 2019（同一台上传器、同一版本，两次现场）：
//   ① pkg/game_web.js, 989:72 → wasm-bindgen 胶水的 ES2021 逻辑赋值 `??=`；
//   ② web-runtime.js, 1:105804 → 仓库自维护运行时的 ES2020 空值合并 `??`。
// 早先只有 ①，于是把档位定在 2020，理由是"同一包里的 `??` 从没被拒过"——那是假象：上传器
// 在 ① 就中止了，根本没走到 ②。② 到手后档位降到 2019，且 acorn 2019 复述出的行列
// （1:105804 `Unexpected token`）与上传器逐字一致，说明这一档就是上传器的解析上限。
// 双向对照（2026-10-03，同一份产物）：2020 档放行整个包却放过了 ② 那个 `??`（这就是漏判），
// 2019 档只在 web-runtime.js 一列报同一处；改掉 `??` 后 2019 档 `upload_ecma_ok files=6`。
// 当前产物的余量（同一批实测）：2018 档整包也过，也就是离上限还有一档；2015 档就撞了
// （`pkg/game_web.js:1250` 与 web-runtime.js 里的 `const`）。
// 哪天上传器证明只接受 2018，把这个数再往下调即可——门把"到底哪一档"留在证据里，而不是
// 留在注释里。
//
// 用法：node scripts/check-upload-ecma.mjs <ecmaVersion> <file.js> [file.js…]
// 输出：`upload_ecma_ok files=N ecma=E`；任何一条不过则逐条打印后 exit 1。
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(here, '..');

function loadAcorn() {
  // acorn 是 terser 的传递依赖，位置随宿主而异：显式 ACORN > engine\.tools 缓存 > 本机 node_modules。
  const bases = [
    process.env.ACORN || '',
    path.join(workspaceRoot, 'engine', '.tools', 'terser'),
    path.join(workspaceRoot, 'engine', '.tools'),
    workspaceRoot,
    here
  ].filter(Boolean);
  for (const base of bases) {
    try {
      const require2 = createRequire(path.join(base, 'noop.js'));
      return require2('acorn');
    } catch (error) { /* 换下一个位置 */ }
  }
  throw new Error('未找到 acorn（terser 的传递依赖）。跑 scripts/minify-toolchain.ps1 装 terser，或设 $env:ACORN=<acorn 目录>');
}

function main(argv) {
  const ecma = Number(argv[0]);
  const files = argv.slice(1);
  if (!Number.isInteger(ecma) || ecma < 2015 || !files.length) {
    process.stderr.write('用法: node scripts/check-upload-ecma.mjs <ecmaVersion(>=2015)> <file.js> [file.js…]\n');
    return 1;
  }
  const acorn = loadAcorn();
  const failures = [];
  for (const file of files) {
    const code = fs.readFileSync(file, 'utf8');
    try {
      // sourceType 必须是 script：上传器与小游戏运行时都不按 ESM 解析（与
      // verify-wechat-minigame.ps1 的"逐脚本按脚本模式解析"同口径）。
      acorn.parse(code, { ecmaVersion: ecma, sourceType: 'script', allowAwaitOutsideFunction: false });
    } catch (error) {
      const pos = typeof error.pos === 'number' ? code.slice(0, error.pos).split('\n').length : 0;
      failures.push(`${path.relative(workspaceRoot, file)}:${pos} ${String(error.message).split('\n')[0]}`);
    }
  }
  if (failures.length) {
    for (const line of failures) process.stdout.write(`upload_ecma_fail ${line}\n`);
    return 1;
  }
  process.stdout.write(`upload_ecma_ok files=${files.length} ecma=${ecma}\n`);
  return 0;
}

process.exitCode = main(process.argv.slice(2));
