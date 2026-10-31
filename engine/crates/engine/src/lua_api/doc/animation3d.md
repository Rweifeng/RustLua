# Lua 模块 `animation3d`

> 运行期 API 面基线里本模块有 7 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `animation3d.crossfade(id: i64, name: String, seconds: f64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:400`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation3d.get(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:367`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

切换骨骼剪辑并做**交叉淡入**：目标剪辑从 0 秒起播，旧剪辑在它自己的时间轴上继续走并在
`seconds` 内淡出；姿态按平移/缩放线性插值、旋转走最短弧的归一化插值。适合 idle↔walk 这类
"同一条骨骼、不同动作"的切换；`animation3d.set` 仍是硬切（时间直接归零）。

- 参数：`id`：实体 id（必须有 `Animation3D` 组件）；`name`：目标剪辑名（来自 `animation3d.list`）；
  `seconds`：淡入时长（秒），`≤ 0` 或 NaN 退化为硬切。
- 返回：`true` 表示当前剪辑已切换；目标就是当前剪辑时返回 `false`（不重启、不淡入）。
- 失败：实体没有 `Animation3D` 报 `entity has no animation3d`；剪辑名未知报 `unknown 3D animation clip`。
- 示例：`animation3d.crossfade(warrior, "Walk", 0.25)`

## `animation3d.list(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:417`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation3d.paused(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:435`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation3d.set(id: i64, name: String)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:383`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation3d.set_paused(id: i64, paused: bool)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:449`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `animation3d.time(id: i64)`

> 绑定：`engine/crates/rustlua-animation/src/animation_lua.rs:465`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
