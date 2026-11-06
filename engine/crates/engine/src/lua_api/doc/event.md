# Lua 模块 `event`

> 运行期 API 面基线里本模块有 5 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `event.emit(name: String, data: Option<Value>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:147`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

同步调用 `name` 上的全部监听者，`data` 原样作为唯一参数（省略时是 `nil`）。
分发使用订阅表的 `Rc` 快照：回调里再 `event.emit` / `event.on` / `event.off` 不会破坏
正在进行的遍历；高频事件应合并，别把它当每帧逐实体的管道。

- 参数：`name` 事件名；`data` 可选载荷。
- 返回：无。
- 失败：任一监听者抛错会中止本次分发，错误原样传给调用者。
- 示例：`event.emit("spawned", { id = entity_id })`。

## `event.off(name: String, callback: Option<Function>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:179`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `event.on(name: String, callback: Function)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:137`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `event.subscribe(name: String, callback: Function)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:128`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `event.unsubscribe(name: String, callback: Option<Function>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/globals.rs:163`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
