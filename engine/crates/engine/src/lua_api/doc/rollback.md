# Lua 模块 `rollback`

> 运行期 API 面基线里本模块有 9 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

预测与回滚账本。它**不碰模拟**：快照是脚本自己序列化的字节串（整张世界表、场景状态都行），
引擎只负责保管最近 `window` 帧的快照/输入、判断"从哪一帧起重模拟"，并统计预测命中情况。
配合 `net.connect_udp` 使用：UDP 中继只保证有序可靠，帧号与输入语义都由脚本定义。

典型每帧顺序（`next_frame → save_snapshot → record_local → inputs → 模拟 → commit`）：

```lua
rollback.reset(2, local_player, 8)
local frame = rollback.next_frame()
rollback.save_snapshot(frame, world.snapshot())
rollback.record_local(frame, encode(local_input))
local inputs = rollback.inputs(frame)   -- 远端缺失处沿用上一帧的输入（预测）
world.step(frame, inputs)
rollback.commit(frame)

-- 收包时：
local kind, from = rollback.accept(player, frame, payload)
if kind == "resimulate" then
  world.restore(rollback.rollback_to(from))
end
```

调用顺序错误（没 `reset` 就调用、提交乱序、回滚到窗口外的帧）一律抛 Lua 错误——
静默返回 `false` 会把"不同步"伪装成"网络慢"。

## `rollback.accept(player: u8, frame: u32, payload: mlua::String)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:95`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

收下一条**远端**输入，并返回账本的判定。

- 参数：
  - `player`：来源玩家号（必须不是 `local`）。
  - `frame`：该输入所属的逻辑帧。
  - `payload`：脚本定义的输入字节串。
- 返回：两个值 `kind, frame?`——
  - `"accepted"`：帧还没模拟过，或与当时预测一致，无需重模拟；
  - `"resimulate", from`：已模拟过的帧收到不同输入，`world.restore(rollback.rollback_to(from))` 后从 `from` 重跑到当前帧；
  - `"toolate", frame`：修正输入早于保留窗口，无法回滚（计入 `stats().stale_inputs`）。
- 失败：未 `reset`、`player` 是本地玩家号时抛错。
- 示例：`local kind, from = rollback.accept(0, frame, payload)`

## `rollback.commit(frame: u32)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:143`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

确认这一帧已模拟完并推进 `next_frame`，同时淘汰窗口外的快照。

- 参数：`frame`：刚刚模拟完的帧。
- 返回：`nil`。
- 失败：帧号不连续（不等于当前 `next_frame`）时抛错。
- 示例：`rollback.commit(frame)`

## `rollback.inputs(frame: u32)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:115`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取这一帧所有玩家要用的输入：已确认的用真值，远端还没到的沿用**上一帧的输入作为预测**。

- 参数：`frame`：要模拟的帧。
- 返回：按玩家号升序的数组 `{ {player=, payload=}, ... }`，长度等于 `reset` 的 `players`。
- 失败：未 `reset` 时抛错。
- 示例：`for _, input in ipairs(rollback.inputs(frame)) do apply(input.player, input.payload) end`

## `rollback.next_frame()`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:76`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

当前应该模拟的帧号（`0` 起；回滚后会把光标退回到 `from`）。

- 参数：无。
- 返回：`number`。
- 失败：未 `reset` 时抛错。
- 示例：`local frame = rollback.next_frame()`

## `rollback.record_local(frame: u32, payload: mlua::String)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:82`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

登记本地玩家这一帧的输入（会和 `inputs()` 里本地那一项一致）。

- 参数：
  - `frame`：逻辑帧号。
  - `payload`：脚本定义的输入字节串。
- 返回：`nil`。
- 失败：同一帧重复登记且内容不同、或帧号早于窗口时抛错。
- 示例：`rollback.record_local(frame, encode(input))`

## `rollback.reset(players: u8, local: u8, window: u32)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:65`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

建/重设账本。热重载保留账本对象，但显式 `reset` 会清空——开新局时调用。

- 参数：
  - `players`：玩家数（≥1，常见 2）。
  - `local`：本地玩家号，必须 `< players`。
  - `window`：保留多少帧的快照用于回滚（≥1）。
- 返回：`nil`。
- 失败：`players == 0`、`local >= players`、`window == 0` 时抛错。
- 示例：`rollback.reset(2, 0, 8)`

## `rollback.rollback_to(from: u32)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:155`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取出 `from` 帧**模拟前**的快照字节，并把 `next_frame` 退到 `from`；调用方负责把世界恢复成这份快照再重跑。

- 参数：`from`：`accept` 返回的重模拟起点。
- 返回：`string`（快照字节，原样返回 `save_snapshot` 存进去的内容）。
- 失败：该帧没有保留下来的快照（早于窗口或从未保存）时抛错。
- 示例：`world.restore(rollback.rollback_to(from))`

## `rollback.save_snapshot(frame: u32, bytes: mlua::String)`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:131`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

保存这一帧**模拟前**的世界快照，供将来回滚；窗口外的旧快照会被自动淘汰。

- 参数：
  - `frame`：逻辑帧号。
  - `bytes`：脚本自己序列化的状态。
- 返回：`nil`。
- 失败：未 `reset` 时抛错。
- 示例：`rollback.save_snapshot(frame, world.snapshot())`

## `rollback.stats()`

> 绑定：`engine/crates/rustlua-gameplay/src/rollback_lua.rs:168`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

回滚账本的累计统计（给 HUD/诊断用，也可在无头对拍里断言）。

- 参数：无。
- 返回：表 `{ resimulations=, resimulated_frames=, stale_inputs=, predicted_inputs= }`。
- 失败：未 `reset` 时抛错。
- 示例：`local s = rollback.stats(); print(s.resimulations, s.stale_inputs)`
