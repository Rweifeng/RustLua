// 微信小游戏入口（game.js）
//
// 加载顺序由小游戏运行时决定，这里显式写清依赖关系：
//   1. wechat-adapter.js  安装浏览器风格宿主 API（window/document/fetch/Image/输入/生命周期）
//   2. adapter.start()    载入 pack/ 资源 → 经宿主的 WXWebAssembly.instantiate(代码包路径,
//                         imports) **异步**实例化 wasm 核心（wasm-bridge.js）→
//                         挂载 window.__RUSTLUA_LUA_BRIDGE → 然后由**适配层 + 宿主**完成启动：
//                         vfs reader 预装（lua_boot_with_reader）+ 引擎执行 main.lua
//                         （lua_execute_main）+ 宿主帧泵与 boot 属性。
//
// WP10 迁移（fengari → mlua-wasm）：Lua 执行层在 wasm 侧 mlua（LuaRuntime 桥），
// pkg/fengari-web.js 已从发布包移除；Lua 交互经 __RUSTLUA_LUA_BRIDGE（lua_boot /
// lua_do_string / lua_set_number / lua_call_global / lua_call_global_void /
// lua_input_frame / input_frame_layout_json / lua_register_js_callback /
// lua_missing_globals / lua_execute_main）完成。
//
// **B8：旧世界 `plugins/webgl/web-runtime.js` 已删除。** 启动编排搬到了适配层
// （`wechat-adapter.js` 的 `hostDirectBoot`）。这里因此**不再 require 任何运行时**，
// 也不再有"跑旧世界"这条分支 —— 那条分支的存在理由（"先注入全部宿主补丁、再执行 main.lua"）
// 随补丁一起消失，因为补丁本身就是旧世界提供的。
'use strict';

var adapter = require('./wechat-adapter.js');

// 启动 Promise 挂到全局，便于自动化验证（scripts/wechat-minigame-harness.js）等待启动完成。
globalThis.__RustLuaWechatBoot = adapter.start(function () {
  // 适配层在宿主直启下**不会**调这个回调（它自己完成启动）。被调了就**响亮失败**，
  // 而不是静默什么都不发生 —— 这是"旧世界已删除"的守卫：谁把它接回来，这里立刻报出来。
  throw new Error('game.js: 旧世界已删除，适配层不应再要求它来启动');
}).catch(function (error) {
  console.error('[game.js] 启动失败', error);
  throw error;
});
