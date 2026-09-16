// 微信小游戏入口（game.js）
//
// 加载顺序由小游戏运行时决定，这里显式写清依赖关系：
//   1. wechat-adapter.js  安装浏览器风格宿主 API（window/document/fetch/Image/输入/生命周期）
//   2. pkg/fengari-web.js Lua VM（UMD：CommonJS 环境下走 module.exports）
//   3. web-runtime.js     与 webgl 发布共用的 Lua 运行时，挂载 window.p2RunLuaGame
//   4. adapter.start()    载入 pack/ 资源 → 注册游戏字体 → 启动 Lua 主循环
//
// 注意：`web-runtime.js` 与 webgl 发布**是同一份文件**（由 scripts/package.ps1 从
// plugins/webgl/ 复制），因此两个平台的游戏逻辑不会各自漂移。
'use strict';

var adapter = require('./wechat-adapter.js');
var fengariModule = require('./pkg/fengari-web.js');

var win = globalThis.window || globalThis;
if (!win.fengari) {
  win.fengari = (fengariModule && fengariModule.lua) ? fengariModule : (fengariModule && fengariModule.default);
}
if (!win.fengari) {
  adapter.showBootError('Lua VM 加载失败：pkg/fengari-web.js 未导出 fengari');
  throw new Error('game.js: fengari 未就绪');
}

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
