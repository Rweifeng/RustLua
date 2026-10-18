# Lua 模块 `app`

> 运行期 API 面基线里本模块有 3 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `app.platform()`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:214`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

返回当前宿主形态的标识串，用来回答"这台设备有没有键盘"。触屏宿主必须常驻屏上摇杆与虚拟按钮，
桌面/手柄宿主则相反——虚拟控件只会盖住画面。**不要**拿"宿主有没有实现 `input.virtual_key`/`ui.circle`"
当"需不需要触屏控件"：那两个 API 由本模块在所有宿主上统一注册，桌面也全有，判能力会恒为真。

- 参数：无
- 返回：`"desktop"`（native 桌面 / 编辑器预览 / headless 验收）或 `"webgl"`（`wasm32-unknown-unknown`，
  浏览器与微信小游戏都构建同一个 `game_web` 源核心；虽然最终 WASM 属于不同平台发布变体，
  但该返回值是同一个 wasm32 编译期常量，故两者同值）
- 失败：不会失败（编译期常量，不查宿主能力）
- 示例：`local touch_host = app.platform() == "webgl"`

## `app.project_root()`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:237`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `app.quit()`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:200`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
