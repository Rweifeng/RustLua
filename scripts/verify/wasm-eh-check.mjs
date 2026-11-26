// Web/微信 wasm 发布 recipe 的 EH 断言：用 **V8 默认参数**编译产物。
//
// 背景（docs/84）：wasm 侧工具链必须留在 rustc 1.88（纯 legacy EH）。≥1.95 的产物是
// "try_table + legacy try" 混合，V8/Edge 会报
// "module uses a mix of legacy and new exception handling instructions" —— 必须在 CI 上守住。
//
// 判据：`new WebAssembly.Module(bytes)` 在**不带 --experimental-wasm-exnref** 的 Node 下成功。
//   - 成功 ⇒ 模块不含 exnref/new-EH 指令（V8 默认打开 legacy EH）；
//   - 失败且信息含 "mix of legacy and new" ⇒ 混合 EH（红）；
//   - 失败且信息含 "invalid opcode 0x1f"/"exnref" ⇒ 用了 new EH（红）。
// 退出码：0 = 通过；1 = 不通过（真红）；2 = 输入缺失（调用方的装配错误）。

import fs from 'node:fs';
import process from 'node:process';

const file = process.argv[2];
if (!file || !fs.existsSync(file)) {
  console.error(`wasm_eh_check_error missing_file=${file}`);
  process.exit(2);
}
const bytes = fs.readFileSync(file);
try {
  // 注意：不能加 --experimental-wasm-exnref；加了就测不出 new EH。
  new WebAssembly.Module(bytes);
} catch (error) {
  const message = String(error && error.message ? error.message : error);
  console.error(`wasm_eh_check_failed file=${file} bytes=${bytes.length} reason=${message}`);
  process.exit(1);
}
console.log(`wasm_eh_ok file=${file} bytes=${bytes.length} mode=legacy`);
