// 微信小游戏入口（game.js）
//
// 加载顺序由小游戏运行时决定，这里显式写清依赖关系：
//   1. wechat-adapter.js  安装浏览器风格宿主 API（window/document/fetch/Image/输入/生命周期）
//   2. web-runtime.js     与 webgl 发布共用的 Lua 运行时，挂载 window.p2RunLuaGame
//   3. adapter.start()    载入 pack/ 资源 → 同步实例化 wasm 核心（wasm-bridge.js）→
//                         挂载 window.__P2_LUA_BRIDGE → 启动 Lua 主循环
//
// WP10 迁移（fengari → mlua-wasm）：Lua 执行层在 wasm 侧 mlua（LuaRuntime 桥），
// pkg/fengari-web.js 已从发布包移除；Lua 交互经 __P2_LUA_BRIDGE（lua_boot /
// lua_do_string / lua_call_global / lua_register_js_callback / lua_missing_globals /
// lua_execute_main）完成。
//
// 注意：`web-runtime.js` 与 webgl 发布**是同一份文件**（由 scripts/package.ps1 从
// plugins/webgl/ 复制），因此两个平台的游戏逻辑不会各自漂移。
'use strict';

var adapter = require('./wechat-adapter.js');

var win = globalThis.window || globalThis;

require('./web-runtime.js');

if (typeof win.p2RunLuaGame !== 'function') {
  adapter.showBootError('运行时未就绪：web-runtime.js 未导出 p2RunLuaGame');
  throw new Error('game.js: p2RunLuaGame 未就绪');
}

// 启动 Promise 挂到全局，便于自动化验证（scripts/wechat-minigame-harness.js）等待启动完成。
globalThis.__RustLuaWechatBoot = adapter.start(function (status) {
  return win.p2RunLuaGame(status);
}).catch(function (error) {
  console.error('[game.js] 启动失败', error);
  throw error;
});
