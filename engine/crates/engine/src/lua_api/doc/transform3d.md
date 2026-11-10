# Lua 模块 `transform3d`

> 运行期 API 面基线里本模块有 7 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `transform3d.attach(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:441`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform3d.get(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:455`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform3d.move(id: i64, dx: f32, dy: f32, dz: f32)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:537`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform3d.position(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:475`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取 3D 实体的**局部**位置，返回 `x, y, z` 三个数，不创建 table；只认 `Transform3D`，
不会把 2D 的 XY 当成 Z=0。

- 参数：`id` 实体句柄。
- 返回：`x: number, y: number, z: number`。
- 失败：死句柄报 `invalid entity id: …`；没有 `Transform3D` 报 `entity has no transform3d`。
- 示例：`local x, y, z = transform3d.position(id)`。

## `transform3d.set(((id, x, y, z, rx, ry, rz, rw, sx, sy, sz): Transform3dSetArgs))`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:497`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform3d.world(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:555`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform3d.world_position(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:484`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取 3D 实体的**世界**位置，返回 `x, y, z` 三个数，不创建 table。已有传播结果时直接取；
没有父节点时局部值就是世界值；只有父链尚未传播时才现场组合一次。语义与 `transform3d.world` 一致。

- 参数：`id` 实体句柄。
- 返回：`x: number, y: number, z: number`。
- 失败：死句柄报 `invalid entity id: …`；没有 `Transform3D` 报 `entity has no transform3d`。
- 示例：`local wx, wy, wz = transform3d.world_position(id)`。
