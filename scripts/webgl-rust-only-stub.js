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
// `start()` 在读到 `?rustlua-gpu=1` 时自己拉起（`game_web/src/lib.rs`），与本文件无关。
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
