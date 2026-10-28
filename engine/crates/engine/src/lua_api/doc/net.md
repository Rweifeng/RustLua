# Lua 模块 `net`

> 运行期 API 面基线里本模块有 8 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

一条到中继服务器（`ClashRoyale/server/server.mjs`）的长连接。桌面走裸 TCP
（`tcp://host:port`），浏览器走 WebSocket（`ws://host:port/ws`），Lua 侧只有这一套面。

服务器把房间配成 2 人：满员时给两端下发 `you:<player>:<players>`，之后二进制消息
原样互转。本模块只搬字节，不管帧同步规则——帧号、输入负载、校验和都由工程脚本
（`scripts/netplay.lua`）定义。

```lua
net.connect(net.default_server(), net.default_room())
-- 之后每帧：local status = net.status(); 在线时 net.send(...) / net.recv()
```

## `net.auto_start()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:614`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

这个进程/页面是否被要求"启动就联机"。桌面读环境变量 `RUSTLUA_NET_AUTO`，
浏览器读查询参数 `?net_auto=1`；取值 `1` / `true` / `yes` / `on` 视为开。

- 参数：无。
- 返回：`boolean`。
- 失败：不会抛错；没有对应环境/参数时返回 `false`。
- 示例：`if net.auto_start() then start_online() end`

## `net.close()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:598`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

主动断开当前连接（没有连接时是空操作）。连接中/等待中/已配对都会转为 `closed`；
已经处于 `error` 时保留错误文本。不会清空已经收到的消息队列。

- 参数：无。
- 返回：`nil`。
- 失败：不会抛错。
- 示例：`net.close()`

## `net.connect(server: String, room: String)`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:553`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

连接中继服务器并加入房间。已有连接会先关闭；房间名必须匹配 `[A-Za-z0-9_-]{1,32}`。
桌面 `server` 形如 `tcp://127.0.0.1:8787`，浏览器形如 `ws://127.0.0.1:8787/ws`
（连接时会把 `?room=` 拼到 URL 上）。

- 参数：
  - `server`：传输地址。
  - `room`：房间码；同一码的两个客户端会被配成对手。
- 返回：`true` 表示连接已建立/已发起；`false` 表示失败，具体原因看 `net.status().error`。
- 失败：地址无法解析、TCP 3 秒内连不上、浏览器拒绝建 WebSocket、房间名非法时返回 `false`，
  状态置为 `error`。
- 示例：`assert(net.connect("tcp://127.0.0.1:8787", "royale"))`

## `net.default_room()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:610`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

默认房间码：桌面取 `RUSTLUA_NET_ROOM`，浏览器取查询参数 `?room=`，都没有时是 `royale`。

- 参数：无。
- 返回：`String`。
- 失败：不会抛错。
- 示例：`local room = net.default_room()`

## `net.default_server()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:606`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

默认服务器地址：桌面取 `RUSTLUA_NET_SERVER`，默认 `tcp://127.0.0.1:8787`；
浏览器取查询参数 `?net_server=`，否则用当前页面的主机名推导 `ws(s)://<host>:8787/ws`
（HTTPS 页面用 `wss`）。

- 参数：无。
- 返回：`String`。
- 失败：不会抛错；页面没有 `window.location` 时退回 `ws://127.0.0.1:8787/ws`。
- 示例：`local server = net.default_server()`

## `net.recv()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:589`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取一条已经到达的二进制消息（FIFO）。空字符串也是一条消息；没有消息时返回 `nil`。
连接关闭后队列里剩余的消息仍可继续取完。

- 参数：无。
- 返回：`string?` —— 消息原始字节；队列空时 `nil`。
- 失败：不会抛错。
- 示例：`local bytes = net.recv()`

## `net.send(bytes: mlua::String)`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:580`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

发送一条二进制消息（对端 `net.recv` 原样收到）。只在 `online` 状态可发；
单帧上限 64 KiB（服务器侧同口径）。

- 参数：
  - `bytes`：任意 Lua 字符串，按字节原样发送。
- 返回：`true` 表示已交给传输层；未配对、已断开或写失败时返回 `false`（写失败会把状态置为 `error`）。
- 失败：不会抛 Lua 错误。
- 示例：`assert(net.send(payload))`

## `net.status()`

> 绑定：`engine/crates/rustlua-gameplay/src/net_lua.rs:560`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取连接状态表，并把传输层已到达的事件并入状态（桌面侧在此刻收事件）。

- 参数：无。
- 返回：表：
  - `state`：`"idle"` / `"connecting"` / `"waiting"`（已连上、等对手）/ `"online"`（已配对）
    / `"closed"` / `"error"`。
  - `player`：配对后本端编号（0 或 1），否则 `nil`。
  - `players`：房间人数（配对后为 2），否则 `nil`。
  - `error`：`closed` / `error` 时的说明文本，否则 `nil`。
  - `pending`：尚未取走的入站消息条数。
  - `server` / `room`：最近一次 `net.connect` 的参数，未连接过时 `nil`。
- 失败：不会抛错。
- 示例：`local s = net.status(); if s.state == "online" then ... end`
