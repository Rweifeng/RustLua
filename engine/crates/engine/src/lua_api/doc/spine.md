# Lua 模块 `spine`

> 运行期 API 面基线里本模块有 8 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

`spine` 是 **Spine 2D 骨骼动画**的运行期接口（M84 P1）。实体上挂 `SpineSkeleton` 组件后，
引擎每帧在 CPU 侧采样骨架并提交三角网格；本表负责在脚本里切换动画、皮肤与速度，并轮询
动画事件。所有函数都以**实体 id** 为第一个参数（`entity.spawn` 一类接口返回的那个 id），
实体上没挂 `SpineSkeleton` 时报错而不是静默返回空值。

事件语义与 `animation.events()` 一致：**取走即清**，一次 `spine.events()` 只返回上次调用
之后新产生的事件；切场景时未取走的事件随旧世界一起作废。

## `spine.bounds(id: i64)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:190`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取本帧骨架网格在**世界空间**的轴对齐包围盒，用于点选、裁剪或血条对齐。

- 参数：`id` — 实体 id。
- 返回：`{ min_x, min_y, max_x, max_y }`（四个浮点数按 1..4 索引）。
- 失败：实体不存在、或没有 `SpineSkeleton` 组件时抛错。骨架尚未采样出顶点（首帧前 / 已禁用）时返回四点全零，可据此判断"暂无包围盒"。
- 示例：`local b = spine.bounds(id); print(b[3] - b[1], b[4] - b[2])`

## `spine.events()`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:172`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取走本帧新触发的动画事件（轮询式，取走即清）。

- 参数：无。
- 返回：数组，每项是 `{ entity = <id>, kind = "start"|"event"|"interrupt"|"end"|"dispose"|"complete", name = <事件名>, animation = <动画名> }`。P1 只产 `start`（切换动画时）；timeline 自定义事件与 `complete` 在 P2 接 listener。
- 失败：无。
- 示例：`for _, e in ipairs(spine.events()) do if e.kind == "start" then log.info(e.animation) end end`

## `spine.get_animation(id: i64)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:77`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读实体当前播放的动画名。

- 参数：`id` — 实体 id。
- 返回：动画名字符串；未设置过动画时返回空串。
- 失败：实体不存在或没有 `SpineSkeleton` 时抛错。
- 示例：`if spine.get_animation(id) ~= "run" then spine.set_animation(id, "run") end`

## `spine.list(id: i64)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:88`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

列出这份骨架里**所有**可用的动画名，用于菜单、调试或做动画存在性判断。

- 参数：`id` — 实体 id。
- 返回：字符串数组（按骨架里的声明顺序）。
- 失败：实体不存在或没有 `SpineSkeleton` 时抛错。
- 示例：`for i, name in ipairs(spine.list(id)) do print(i, name) end`

## `spine.set_animation(id: i64, name: String, looped: Option<bool>)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:104`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

切换实体正在播放的动画。

- 参数：`id` — 实体 id；`name` — 动画名，必须是 `spine.list` 里的一个；`looped` — 是否循环，缺省 `true`。传空串表示停止播放当前位置。
- 返回：无。
- 失败：动画名不存在时抛错（不做"忽略并继续播旧的"这种静默回退）；实体没有骨架时抛错。
- 示例：`spine.set_animation(id, "attack", false)`

## `spine.set_enabled(id: i64, enabled: bool)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:156`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

开关这个实体的 Spine 推进与更新。

- 参数：`id` — 实体 id；`enabled` — `false` 时停止推进动画，但**保留上一帧网格**（不会闪没）。
- 返回：无。
- 失败：实体没有 `SpineSkeleton` 时抛错。
- 示例：`spine.set_enabled(id, false) -- 暂停但画面保留`

## `spine.set_skin(id: i64, name: String)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:122`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

切换骨架皮肤（换装 / 换武器外观）。

- 参数：`id` — 实体 id；`name` — 皮肤名，缺省皮肤叫 `"default"`。
- 返回：无。
- 失败：皮肤名不存在时抛错。
- 示例：`spine.set_skin(id, "armor_heavy")`

## `spine.set_speed(id: i64, speed: f32)`

> 绑定：`engine/crates/rustlua-spine/src/spine_lua.rs:140`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

设置动画播放速度倍率。

- 参数：`id` — 实体 id；`speed` — 倍率，`1.0` 为原始速度，`0.0` 等效冻结但**仍会重新采样**（与 `set_enabled(false)` 不同，前者可用于逐帧精确定格）。负数或非有限值被忽略。
- 返回：无。
- 失败：实体没有 `SpineSkeleton` 时抛错。
- 示例：`spine.set_speed(id, 0.5) -- 半速`
