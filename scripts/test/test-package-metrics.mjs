#!/usr/bin/env node
/**
 * test-package-metrics.mjs —— measure-artifacts --kind package 自测（P4-07）。
 * 用临时工程目录造 主包+分包+字体+wasm，断言：
 *   1. wechat 主包/分包分账正确（主包 = launcher 总 - 分包）
 *   2. 配额余量正确（main_of_limit/total_of_limit）
 *   3. 缺 game.json -> partial=true（不能拿 partial 更新基线）
 *   4. webgl wasm/js/字体分账存在
 * 输出：package_metrics_selftest_ok cases=4
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(__dirname, "..", "metrics", "measure-artifacts.mjs");

function mk(size) {
  return Buffer.alloc(size, 0x41); // 'A'
}

function main() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "pkg-metrics-test-"));
  const project = path.join(tmp, "game_jhyt");
  const launcher = path.join(project, "builds", "wechat-minigame", "launcher");
  const webgl = path.join(project, "builds", "webgl");
  const webglPkg = path.join(webgl, "pkg");
  const subDir = path.join(launcher, "pack-sub");
  const subDir2 = path.join(launcher, "pack-wasm");
  let failed = 0;

  try {
    fs.mkdirSync(webglPkg, { recursive: true });
    fs.writeFileSync(path.join(webglPkg, "game_web_bg.wasm"), mk(100_000));
    fs.writeFileSync(path.join(webglPkg, "game_web.js"), mk(20_000));
    fs.writeFileSync(path.join(webgl, "font.ttf"), mk(8_000));

    fs.mkdirSync(subDir, { recursive: true });
    fs.writeFileSync(path.join(subDir, "a.js"), mk(50_000));
    fs.mkdirSync(subDir2, { recursive: true });
    fs.writeFileSync(path.join(subDir2, "game.wasm"), mk(200_000));
    fs.writeFileSync(path.join(launcher, "game.js"), mk(30_000));
    fs.writeFileSync(path.join(launcher, "game.json"), JSON.stringify({
      subpackages: [{ name: "pack-sub", root: "pack-sub" }, { name: "pack-wasm", root: "pack-wasm" }],
    }));
    fs.writeFileSync(path.join(launcher, "code-package.json"), "{}");

    const out = path.join(tmp, "packages.json");
    let stdout;
    try {
      stdout = execFileSync(process.execPath, [script, "--kind", "package", "--project", project, "--out", out], { encoding: "utf8", timeout: 60000 });
    } catch (e) {
      console.error("[FAIL] measure-artifacts 运行失败: " + e.message);
      return 1;
    }
    const data = JSON.parse(fs.readFileSync(out, "utf8"));
    const main = data.packages.find((p) => p.platform === "wechat" && p.name === "main");
    const subs = data.packages.filter((p) => p.platform === "wechat" && p.name.startsWith("sub|"));
    const webglPkgEntry = data.packages.find((p) => p.platform === "webgl");

    // 期望：分包 50_000 + 200_000 = 250_000；主包 = (30_000 + 8_000 字 + game.json 等) = 总 - 分包
    const subTotal = subs.reduce((a, b) => a + b.bytes_total, 0);
    if (subTotal === 250_000) console.log("[PASS] 分包分账 = 250000");
    else { console.log("[FAIL] 分包分账: " + subTotal); failed++; }

    const mainBytes = main.bytes_main;
    if (mainBytes > 0 && mainBytes === main.bytes_total - subTotal) console.log("[PASS] 主包分账 = 总 - 分包");
    else { console.log(`[FAIL] 主包分账: main=${mainBytes} total=${main.bytes_total} sub=${subTotal}`); failed++; }

    if (main.main_of_limit <= 100 && main.total_of_limit <= 100) console.log("[PASS] 配额余量正确");
    else { console.log("[FAIL] 配额余量: " + main.main_of_limit + "/" + main.total_of_limit); failed++; }

    if (webglPkgEntry.bytes_wasm === 100_000 && webglPkgEntry.bytes_js === 20_000 && webglPkgEntry.bytes_fonts === 8_000) console.log("[PASS] webgl wasm/js/字体分账");
    else { console.log(`[FAIL] webgl 分账: wasm=${webglPkgEntry.bytes_wasm} js=${webglPkgEntry.bytes_js} fonts=${webglPkgEntry.bytes_fonts}`); failed++; }

    // 缺 game.json -> partial
    const project2 = path.join(tmp, "game_nogamejson");
    fs.mkdirSync(path.join(project2, "builds", "wechat-minigame"), { recursive: true });
    const out2 = path.join(tmp, "packages2.json");
    let stdout2;
    try {
      stdout2 = execFileSync(process.execPath, [script, "--kind", "package", "--project", project2, "--out", out2], { encoding: "utf8", timeout: 60000 });
    } catch (e) {
      console.error("[FAIL] 缺 game.json 用例运行失败: " + e.message);
      return 1;
    }
    const data2 = JSON.parse(fs.readFileSync(out2, "utf8"));
    if (data2.partial === true) console.log("[PASS] 缺 game.json -> partial=true（不能更新基线）");
    else { console.log("[FAIL] 缺 game.json 应 partial=true"); failed++; }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }

  console.log("");
  if (failed > 0) {
    console.log(`package_metrics_selftest_failed cases=5 failures=${failed}`);
    process.exit(1);
  }
  console.log("package_metrics_selftest_ok cases=5");
  process.exit(0);
}

main();
