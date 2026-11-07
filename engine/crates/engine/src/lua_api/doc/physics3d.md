# Lua 模块 `physics3d`

> 运行期 API 面基线里本模块有 10 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `physics3d.add_heightfield(id: i64, x: f32, y: f32, z: f32, rows: u32, cols: u32, sx: f32, sy: f32, sz: f32, heights: Table)`

为实体添加一块**静态地形高度场**（P3-R55）。`heights` 行主序、`rows × cols` 个数；
局部 XZ 由 `sx/sz` 铺开，高度乘 `sy`。高度场是固定体，可被射线/动态刚体碰撞，
不参与 `physics3d.debug_draw` 的线框（不是简单形状）。

- 参数：`rows/cols` `2..=1025` 且 `heights` 长度必须等于 `rows*cols`；`sx/sy/sz > 0`。
- 返回：无。
- 失败：实体不存在/已有刚体、行数/高度数/尺度非法。
- 示例：
```lua
local heights = {}
for i = 1, 33 * 33 do heights[i] = 0 end
physics3d.add_heightfield(terrain_entity, 0, 0, 0, 33, 33, 480, 1, 480, heights)
```

## `physics3d.debug_draw(enabled: bool)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:166`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.events()`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:143`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.get_ccd(id: i64)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:123`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.get_velocity(id: i64)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:367`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.overlap(cx: f32, cy: f32, cz: f32, radius: f32, opts: Option<Table>)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:476`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.raycast(ox: f32, oy: f32, oz: f32, dx: f32, dy: f32, dz: f32, max_distance: f32, opts: Option<Table>)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:427`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_ccd(id: i64, enabled: bool)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:105`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_position(id: i64, x: f32, y: f32, z: f32)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:393`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_velocity(((id $(, $component)+)))`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:184`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
