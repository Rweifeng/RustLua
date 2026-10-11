// Rust-only 宿主探针的 web-runtime 替身（M73-00 WP6-03）。
//
// 为什么是这个形状：产物里的 `index.html` 启动 Rust 侧之后，**显式**调一次
// `window.rustluaRunLuaGame(status)` 来启动 JS 世界（web-runtime.js + 补丁层）。
// 也就是说"要不要跑 JS 世界"是启动链上的一个显式决定，而不是只能靠改 `index.html`。
//
// 于是做"Rust 单跑"这件事不需要动任何用户文件：把 184 KB 的 `web-runtime.js` 换成这个替身，
// `index.html` 一行不改、wasm 一行不改、pack 一项不改——跑起来的就是**只有 Rust 的那条路**。
//
// `index.html` 还会用到 `window.__rustluaSyncCanvasSize`，那个函数定义在 `index.html` 自己的
// 内联脚本里（第 24 行），所以这里**不需要**提供。GPU 宿主也不是这里启动的：它由 Rust 的
// `start()` 自己拉起（`game_web/src/lib.rs`，默认开、`?rustlua-gpu=0` 关），与本文件无关。
//
// M73-00 WP8：`index.html` 现在会在"探不到 GPU 宿主"（wasm 导出里没有 `gpu_host_run`）且
// 没显式 `?rustlua-js-host=0` 时**退回**本函数，避免默认页面没有绘制者。这个替身照样是空实现
// ⇒ 那种组合（非 `-Gpu2d` 产物 + `-StubJsRuntime`）仍然是"只有 Rust、且 Rust 不画"，
// 正是这个探针想制造的形态；日志里那行 `[probe] rust-only host:` 就是它被调到的凭据。
(function () {
  'use strict';
  window.__RUSTLUA_RUST_ONLY_HOST = true;
  // 有意留空：不建世界、不注册 Lua 桥回调、不画 Canvas2D。
  // 这个探针要问的正是"没有 JS 世界时，Rust 那条路自己能不能把画面出全"。
  window.rustluaRunLuaGame = async function () {
    if (window.console && window.console.log) {
      window.console.log('[probe] rust-only host: JS 世界未启动（web-runtime.js 已换成替身）');
    }
  };
})();
