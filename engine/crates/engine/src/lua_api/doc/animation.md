# Lua 模块 `animation`

> 运行期 API 面基线里本模块有 13 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `animation.attach(id: i64, component: String)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:198`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.clips(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:214`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.ended(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:180`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.events()`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:237`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.frame(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:163`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.get(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:129`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.pause(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:293`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

暂停 `.anim` 时间线播放器（`AnimationPlayer`），保留当前时刻；暂停期间仍按当前时刻采样，`seek` 后的姿态会立刻生效。

- 参数：`id` 实体句柄。
- 返回：`true`。
- 失败：死句柄报 `invalid entity id`；实体没有 `AnimationPlayer` 报 `entity has no animation player`。
- 示例：`animation.pause(hero)`

## `animation.play(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:277`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

从当前时刻恢复 `.anim` 时间线播放（清掉播放结束标记）；`autoplay=false` 的播放器需要它来起步。

- 参数：`id` 实体句柄。
- 返回：`true`。
- 失败：死句柄报 `invalid entity id`；实体没有 `AnimationPlayer` 报 `entity has no animation player`。
- 示例：`animation.play(hero)`

## `animation.player_events()`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:345`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取走本帧 `.anim` 播放器触发的事件（一次性队列，取走即清）；与旧状态机的帧事件 `animation.events()` 是两条独立队列。

- 参数：无。
- 返回：事件数组，每条为 `{entity, name, time}`（`time` 是事件在时间轴上的秒数）；没有事件时为空表。
- 失败：宿主正持有动画状态的可变借用时报 `already borrowed`。
- 示例：`for _, e in ipairs(animation.player_events()) do log(e.name, e.time) end`

## `animation.seek(id: i64, seconds: f64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:325`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把 `.anim` 播放器定位到指定秒数：循环剪辑折回 `[0, duration)`，非循环夹到 `[0, duration]`，并清掉结束标记。

- 参数：`id` 实体句柄；`seconds` 目标时刻（秒，必须有限）。
- 返回：`true`。
- 失败：死句柄报 `invalid entity id`；实体没有 `AnimationPlayer` 报 `entity has no animation player`；`seconds` 非有限报 `seek time must be finite`；播放器没有已加载片段报 `player has no loaded clip`。
- 示例：`animation.seek(hero, 0.75)`

## `animation.set(id: i64, name: String)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:146`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation.stop(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:309`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

停止 `.anim` 播放并把时间归零（下一次 `play` 从片头开始），同时清掉结束标记。

- 参数：`id` 实体句柄。
- 返回：`true`。
- 失败：死句柄报 `invalid entity id`；实体没有 `AnimationPlayer` 报 `entity has no animation player`。
- 示例：`animation.stop(hero)`

## `animation.time(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:257`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
